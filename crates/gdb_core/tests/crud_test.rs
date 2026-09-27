//! 增删改（CRUD）能力的真实 `.gdb` 集成测试（fixture: `tests/fixtures/test.gdb`）。
//!
//! 所有会写盘的用例都在**临时副本**上进行（风格对齐 `real_gdb_test.rs::update_on_real_copy`），
//! 操作后重新打开校验，不触碰只读 fixture。
//!
//! 关键金标准：**OBJECTID = 槽位 + 1**，删除采用「保槽删除」，因此删除若干要素后，
//! 其余要素的 OBJECTID 必须保持不变——这是 FileGDB 的语义，也是本文件最重要的断言。

use gdb_core::cursor::QueryFilter;
use gdb_core::field::FieldType;
use gdb_core::geometry::{Geometry, Polygon};
use gdb_core::table::Table;
use gdb_core::value::FieldValue;
use gdb_core::workspace::Geodatabase;
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test.gdb")
}

/// 把 fixture 复制到临时目录（每个用例独立子目录，避免相互干扰）。
fn copy_fixture(tag: &str) -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("gdb_rs_crud_{tag}"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    for entry in std::fs::read_dir(fixture()).unwrap() {
        let entry = entry.unwrap();
        let dst = tmp.join(entry.file_name());
        std::fs::copy(entry.path(), dst).unwrap();
    }
    tmp
}

/// 读取指定要素类的全部 OID（升序）。
fn oids_of(gdb: &Geodatabase, name: &str) -> Vec<u64> {
    let fc = gdb.open_feature_class(name).unwrap();
    let mut cur = fc.search(&QueryFilter::All).unwrap();
    let mut v = Vec::new();
    while let Some(f) = cur.next_feature() {
        v.push(f.object_id());
    }
    v.sort_unstable();
    v
}

/// 计算几何总周长（与 real_gdb_test 一致，用于几何回读金标准）。
fn geometry_length(g: &Geometry) -> f64 {
    let pts: Vec<Vec<(f64, f64)>> = match g {
        Geometry::Polygon(p) => p.rings.clone(),
        Geometry::Polyline(p) => p.parts.clone(),
        _ => return 0.0,
    };
    let mut total = 0.0;
    for part in pts {
        for w in part.windows(2) {
            let dx = w[1].0 - w[0].0;
            let dy = w[1].1 - w[0].1;
            total += (dx * dx + dy * dy).sqrt();
        }
    }
    total
}

// ---------------------------------------------------------------------------
// 删除
// ---------------------------------------------------------------------------

#[test]
fn delete_by_where_on_dikuai() {
    let tmp = copy_fixture("del_where");
    let gdb = Geodatabase::open(&tmp).unwrap();
    assert_eq!(oids_of(&gdb, "地块"), vec![2, 3, 4, 5]);

    let fc = gdb.open_feature_class("地块").unwrap();
    let filter = QueryFilter::where_clause("BH = '编号1'").unwrap();
    let n = fc.delete_searched_rows(&filter).unwrap();
    assert_eq!(n, 1, "应删除 1 个要素");
    fc.save().unwrap();
    drop(fc);
    drop(gdb);

    // 重新打开：数量减 1，其余 OID 完全不变（保槽删除）。
    let gdb2 = Geodatabase::open(&tmp).unwrap();
    assert_eq!(oids_of(&gdb2, "地块"), vec![3, 4, 5], "其余 OID 必须不变");
    // 被删的 OID=2（槽 1）在 .gdbtablx 中应为空槽
    let t = Table::open(&tmp, 9).unwrap();
    assert!(!t.row_slots.contains(&1), "槽 1 应为空槽");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn delete_single_oid_stability() {
    let tmp = copy_fixture("del_oid");
    let gdb = Geodatabase::open(&tmp).unwrap();
    let fc = gdb.open_feature_class("地块").unwrap();
    assert!(fc.delete_feature(3).unwrap(), "OID=3 应存在");
    assert!(!fc.delete_feature(999).unwrap(), "不存在的 OID 应返回 false");
    fc.save().unwrap();
    drop(fc);
    drop(gdb);

    let gdb2 = Geodatabase::open(&tmp).unwrap();
    assert_eq!(oids_of(&gdb2, "地块"), vec![2, 4, 5]);
    assert_eq!(gdb2.open_feature_class("地块").unwrap().feature_count(), 3);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn delete_rows_batch() {
    let tmp = copy_fixture("del_batch");
    let gdb = Geodatabase::open(&tmp).unwrap();
    let fc = gdb.open_feature_class("地块").unwrap();
    let n = fc.delete_rows(&[2, 4, 12345]).unwrap();
    assert_eq!(n, 2, "仅 2 个存在");
    fc.save().unwrap();
    drop(fc);
    drop(gdb);

    let gdb2 = Geodatabase::open(&tmp).unwrap();
    assert_eq!(oids_of(&gdb2, "地块"), vec![3, 5]);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn delete_all_by_where() {
    let tmp = copy_fixture("del_all");
    let gdb = Geodatabase::open(&tmp).unwrap();
    let fc = gdb.open_feature_class("地块").unwrap();
    let n = fc
        .delete_searched_rows(&QueryFilter::where_clause("Shape_Length > 0").unwrap())
        .unwrap();
    assert_eq!(n, 4);
    fc.save().unwrap();
    drop(fc);
    drop(gdb);

    let gdb2 = Geodatabase::open(&tmp).unwrap();
    assert_eq!(gdb2.open_feature_class("地块").unwrap().feature_count(), 0);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn cursor_delete_feature() {
    let tmp = copy_fixture("cur_del");
    let gdb = Geodatabase::open(&tmp).unwrap();
    let fc = gdb.open_feature_class("地块").unwrap();
    // 先 Search 收集，再 Update 删除（正确范式）。
    let mut victims = Vec::new();
    {
        let mut sc = fc.search(&QueryFilter::where_clause("BH = '编号2'").unwrap()).unwrap();
        while let Some(f) = sc.next_feature() {
            victims.push(f);
        }
    }
    assert_eq!(victims.len(), 1);
    let mut uc = fc.update(&QueryFilter::All).unwrap();
    for f in &victims {
        uc.delete_feature(f).unwrap();
    }
    fc.save().unwrap();
    drop(fc);
    drop(gdb);

    let gdb2 = Geodatabase::open(&tmp).unwrap();
    assert_eq!(oids_of(&gdb2, "地块"), vec![2, 4, 5]);
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------
// 插入 / 创建
// ---------------------------------------------------------------------------

#[test]
fn create_feature_on_dikuai_hole() {
    // 回归：地块表槽 0 为空槽（OID 从 2 开始），插入新行必须落在槽位 slot_count
    // 且 OBJECTID == 槽位 + 1，并能被 row_index_by_oid 命中。
    let tmp = copy_fixture("create_hole");
    let gdb = Geodatabase::open(&tmp).unwrap();
    let fc = gdb.open_feature_class("地块").unwrap();

    // 打开底层表确认初始槽位状态。
    let t0 = Table::open(&tmp, 9).unwrap();
    assert_eq!(t0.rows.len(), 4, "4 个有效行");
    assert_eq!(t0.slot_count, 5, "槽位总数应为 5（含 1 空槽）");
    drop(t0);

    let fields = fc.fields();
    let mut values = vec![FieldValue::Null; fields.len()];
    let oi = fields.iter().position(|f| f.field_type == FieldType::ObjectId).unwrap();
    values[oi] = FieldValue::ObjectId(0); // 占位
    let bi = fields.iter().position(|f| f.name == "BH").unwrap();
    values[bi] = FieldValue::Text("编号9".into());
    let gi = fields.iter().position(|f| f.field_type == FieldType::Geometry).unwrap();
    values[gi] = FieldValue::Geometry(Geometry::Polygon(Polygon {
        rings: vec![vec![
            (40538389.490, 3044658.394),
            (40538499.680, 3044695.035),
            (40538449.010, 3044778.332),
            (40538389.490, 3044742.125),
            (40538389.490, 3044658.394),
        ]],
    }));

    let new_oid = fc.create_feature(values).unwrap();
    assert_eq!(new_oid, 6, "新 OID = slot_count(5) + 1 = 6");
    assert_eq!(fc.feature_count(), 5);
    // 新行可被 OID 命中
    assert!(fc.get_feature(new_oid).unwrap().is_some());
    fc.save().unwrap();
    drop(fc);
    drop(gdb);

    // 重新打开校验 OID 与几何
    let t = Table::open(&tmp, 9).unwrap();
    let idx = t.row_index_by_oid(6).expect("OID=6 应可命中");
    let bh = t.schema.field_index("BH").unwrap();
    assert_eq!(t.rows[idx][bh], FieldValue::Text("编号9".into()));
    let gi = t.schema.geometry_index().unwrap();
    match &t.rows[idx][gi] {
        FieldValue::Geometry(g) => {
            // 回读几何应与写入的环一致（周长金标准，容差 1e-3）。
            let expect = 367.01885752436857;
            assert!(
                (geometry_length(g) - expect).abs() < 1e-3,
                "几何应可回读，实际周长 {}",
                geometry_length(g)
            );
            match g {
                Geometry::Polygon(pg) => {
                    assert_eq!(pg.rings.len(), 1);
                    assert_eq!(pg.rings[0].len(), 5, "闭合环 5 点");
                    assert!((pg.rings[0][0].0 - 40538389.490).abs() < 1e-3);
                }
                other => panic!("应为面几何: {other:?}"),
            }
        }
        other => panic!("应为几何: {other:?}"),
    }
    assert_eq!(t.rows.len(), 5);
    assert_eq!(t.row_slots, vec![1, 2, 3, 4, 5], "新行落在槽 5");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn delete_all_then_insert_no_slot_reuse() {
    // 删光后插入：新 OID 不复用已空出的槽位，而是继续向后分配。
    let tmp = copy_fixture("reuse");
    let gdb = Geodatabase::open(&tmp).unwrap();
    let fc = gdb.open_feature_class("地块").unwrap();
    fc.delete_searched_rows(&QueryFilter::All).unwrap();
    assert_eq!(fc.feature_count(), 0);

    let fields = fc.fields();
    let mut values = vec![FieldValue::Null; fields.len()];
    let bi = fields.iter().position(|f| f.name == "BH").unwrap();
    values[bi] = FieldValue::Text("新地".into());
    let new_oid = fc.create_feature(values).unwrap();
    assert_eq!(new_oid, 6, "不复用空槽，继续分配 OID=6");
    fc.save().unwrap();
    drop(fc);
    drop(gdb);

    let gdb2 = Geodatabase::open(&tmp).unwrap();
    assert_eq!(oids_of(&gdb2, "地块"), vec![6]);
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------
// 更新
// ---------------------------------------------------------------------------

#[test]
fn update_by_where_batch() {
    let tmp = copy_fixture("upd_where");
    let gdb = Geodatabase::open(&tmp).unwrap();
    let session = gdb.edit_session();
    session.start();
    session.start_operation().unwrap();

    let fc = session.open_feature_class(&gdb, "地块").unwrap();
    let filter = QueryFilter::where_clause("BH = '编号2' OR BH = '编号3'").unwrap();
    let mut cur = fc.update(&filter).unwrap();
    let mut n = 0;
    while let Some(f) = cur.next_feature() {
        f.set_by_name("BH", FieldValue::Text("批量改".into())).unwrap();
        f.store();
        cur.update(f.row()).unwrap();
        n += 1;
    }
    assert_eq!(n, 2, "应命中 2 个");
    session.stop_operation().unwrap();
    session.commit().unwrap();
    drop(session);
    drop(gdb);

    let gdb2 = Geodatabase::open(&tmp).unwrap();
    let fc2 = gdb2.open_feature_class("地块").unwrap();
    let mut cur = fc2.search(&QueryFilter::where_clause("BH = '批量改'").unwrap()).unwrap();
    let mut hits = 0;
    while let Some(f) = cur.next_feature() {
        hits += 1;
        // OID 保持不变（更新不改变槽位）
        assert!(f.object_id() == 3 || f.object_id() == 4);
    }
    assert_eq!(hits, 2);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn update_by_where_fwdealdata() {
    let tmp = copy_fixture("upd_fw");
    let gdb = Geodatabase::open(&tmp).unwrap();
    let session = gdb.edit_session();
    session.start();
    session.start_operation().unwrap();

    // fw_DealData 6151 要素，province 全为「山西省」：批量把 province 改为「山西省-改」。
    let fc = session.open_feature_class(&gdb, "fw_DealData").unwrap();
    let fields = fc.fields();
    let prov_idx = fields.iter().position(|f| f.name == "province").unwrap();
    // 仅选取前 5 行演示（避免测试过慢）；用 OID 集合过滤。
    let first_oids: Vec<u64> = {
        let mut sc = fc.search(&QueryFilter::All).unwrap();
        let mut v = Vec::new();
        while let Some(f) = sc.next_feature() {
            v.push(f.object_id());
            if v.len() >= 5 {
                break;
            }
        }
        v
    };
    let _ = prov_idx;
    let filter = QueryFilter::oids(first_oids.clone());
    let mut cur = fc.update(&filter).unwrap();
    let mut n = 0;
    while let Some(f) = cur.next_feature() {
        f.set_by_name("province", FieldValue::Text("山西省-改".into()))
            .unwrap();
        f.store();
        cur.update(f.row()).unwrap();
        n += 1;
    }
    assert_eq!(n, 5);
    session.stop_operation().unwrap();
    session.commit().unwrap();
    drop(session);
    drop(gdb);

    let gdb2 = Geodatabase::open(&tmp).unwrap();
    let fc2 = gdb2.open_feature_class("fw_DealData").unwrap();
    assert_eq!(fc2.feature_count(), 6151, "总数不变");
    let mut cur = fc2
        .search(&QueryFilter::where_clause("province = '山西省-改'").unwrap())
        .unwrap();
    let mut hits = 0;
    while cur.next_feature().is_some() {
        hits += 1;
    }
    assert_eq!(hits, 5, "应恰好改到 5 行");
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------
// QueryFilter 变体
// ---------------------------------------------------------------------------

#[test]
fn query_filter_oids_variant() {
    let tmp = copy_fixture("qf_oids");
    let gdb = Geodatabase::open(&tmp).unwrap();
    let fc = gdb.open_feature_class("地块").unwrap();
    let got = fc.query_oids(&QueryFilter::oids([2, 5])).unwrap();
    assert_eq!(got, vec![2, 5]);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn where_or_and_paren() {
    let tmp = copy_fixture("qf_where");
    let gdb = Geodatabase::open(&tmp).unwrap();
    let fc = gdb.open_feature_class("地块").unwrap();
    let got = fc
        .query_oids(
            &QueryFilter::where_clause("(BH = '编号1' OR BH = '编号2') AND Shape_Length > 0")
                .unwrap(),
        )
        .unwrap();
    assert_eq!(got, vec![2, 3], "应命中 OID 2 与 3");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn spatial_filter_exact_on_dikuai() {
    let tmp = copy_fixture("qf_spatial");
    let gdb = Geodatabase::open(&tmp).unwrap();
    let fc = gdb.open_feature_class("地块").unwrap();

    // 取 OID=2 的几何作为查询框（自身必然相交）。
    let f2 = fc.get_feature(2).unwrap().unwrap();
    let g2 = f2.geometry().unwrap();
    let env = g2.envelope().unwrap();

    // 用「包围 OID=2 的小框」做 Intersects：至少命中 OID=2。
    let qbox = Geometry::Polygon(Polygon {
        rings: vec![vec![
            (env.0 + 1.0, env.1 + 1.0),
            (env.2 - 1.0, env.1 + 1.0),
            (env.2 - 1.0, env.3 - 1.0),
            (env.0 + 1.0, env.3 - 1.0),
        ]],
    });
    let hit = fc
        .query_oids(&QueryFilter::spatial(gdb_core::SpatialRel::Intersects, qbox).unwrap())
        .unwrap();
    assert!(hit.contains(&2), "OID=2 应被其内部小框命中: {hit:?}");

    // 远处小框：应 0 命中。
    let far = Geometry::Polygon(Polygon {
        rings: vec![vec![
            (0.0, 0.0),
            (1.0, 0.0),
            (1.0, 1.0),
            (0.0, 1.0),
        ]],
    });
    let none = fc
        .query_oids(&QueryFilter::spatial(gdb_core::SpatialRel::Intersects, far).unwrap())
        .unwrap();
    assert!(none.is_empty(), "远离数据的小框不应命中: {none:?}");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn spatial_filter_exact_excludes_hole() {
    // 证明空间过滤是**精确几何判定**而非 bbox 近似：
    // 构造一个「外环包住某要素 bbox + 内环（洞）覆盖该要素中心」的查询面。
    // bbox 近似会命中该要素；精确判定（奇偶规则含洞）必须不命中。
    let tmp = copy_fixture("qf_fp");
    let gdb = Geodatabase::open(&tmp).unwrap();
    let fc = gdb.open_feature_class("地块").unwrap();
    let f2 = fc.get_feature(2).unwrap().unwrap();
    let g = f2.geometry().unwrap();
    let env = g.envelope().unwrap();

    let margin = 10.0;
    let outer = vec![
        (env.0 - margin, env.1 - margin),
        (env.2 + margin, env.1 - margin),
        (env.2 + margin, env.3 + margin),
        (env.0 - margin, env.3 + margin),
        (env.0 - margin, env.1 - margin),
    ];
    // 洞覆盖 OID=2 的整个 bbox（略大一点），使 OID=2 落在洞内。
    let hole = vec![
        (env.0 - 1.0, env.1 - 1.0),
        (env.2 + 1.0, env.1 - 1.0),
        (env.2 + 1.0, env.3 + 1.0),
        (env.0 - 1.0, env.3 + 1.0),
        (env.0 - 1.0, env.1 - 1.0),
    ];
    let donut = Geometry::Polygon(Polygon {
        rings: vec![outer, hole],
    });

    // bbox 层面：查询面的 bbox 完全覆盖 OID=2 的 bbox（近似会命中）。
    let q_env = donut.envelope().unwrap();
    assert!(
        q_env.0 <= env.0 && q_env.1 <= env.1 && q_env.2 >= env.2 && q_env.3 >= env.3,
        "查询面 bbox 应覆盖 OID=2 的 bbox"
    );

    // 精确判定：OID=2 位于洞内 → 不应命中。
    let hit = fc
        .query_oids(&QueryFilter::spatial(gdb_core::SpatialRel::Intersects, donut).unwrap())
        .unwrap();
    assert!(
        !hit.contains(&2),
        "OID=2 位于查询面之洞内，精确判定不应命中（bbox 近似会误判）: {hit:?}"
    );
    // 其余要素离得很远（margin 只包住 OID=2），故应全部不命中。
    assert!(hit.is_empty(), "小范围内仅 OID=2，且其在洞内 → 应无命中: {hit:?}");
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------
// EditSession
// ---------------------------------------------------------------------------

#[test]
fn edit_session_abort_rollback() {
    let tmp = copy_fixture("es_abort");
    let gdb = Geodatabase::open(&tmp).unwrap();
    let session = gdb.edit_session();
    session.start();
    session.start_operation().unwrap();

    let fc = session.open_feature_class(&gdb, "地块").unwrap();
    // 删 1 个 + 改 1 个
    fc.delete_feature(2).unwrap();
    {
        let mut cur = fc.update(&QueryFilter::ByOid(3)).unwrap();
        let f = cur.next_feature().unwrap();
        f.set_by_name("BH", FieldValue::Text("被改".into())).unwrap();
        f.store();
        cur.update(f.row()).unwrap();
    }
    assert_eq!(fc.feature_count(), 3, "内存中已删除");

    session.abort();

    // abort 后内存应回滚
    assert_eq!(fc.feature_count(), 4, "abort 应回滚删除");
    let mut cur = fc.search(&QueryFilter::ByOid(3)).unwrap();
    let f = cur.next_feature().unwrap();
    assert_eq!(f.get_by_name("BH").unwrap(), FieldValue::Text("编号2".into()));

    drop(session);
    drop(fc);
    drop(gdb);

    // 磁盘未变
    let gdb2 = Geodatabase::open(&tmp).unwrap();
    assert_eq!(oids_of(&gdb2, "地块"), vec![2, 3, 4, 5]);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn edit_session_nested_operations() {
    let tmp = copy_fixture("es_nested");
    let gdb = Geodatabase::open(&tmp).unwrap();
    let session = gdb.edit_session();
    session.start();
    session.start_operation().unwrap();
    session.start_operation().unwrap();
    assert_eq!(session.operation_depth(), 2);

    session.stop_operation().unwrap();
    // 仍有 1 层未结束 → commit 应报错
    assert!(session.commit().is_err(), "存在未结束操作时 commit 必须失败");

    session.stop_operation().unwrap();
    assert_eq!(session.operation_depth(), 0);
    session.commit().unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn edit_session_commit_guard() {
    let tmp = copy_fixture("es_guard");
    let gdb = Geodatabase::open(&tmp).unwrap();
    let session = gdb.edit_session();
    // 未 start 就 commit → 报错
    assert!(session.commit().is_err());
    assert!(!session.is_being_edited());
    // 未 start 就 start_operation → 报错
    assert!(session.start_operation().is_err());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn edit_session_abort_without_operation_keeps_memory() {
    // 未调用 start_operation 时 abort 退化为「不写盘」，内存改动保留（与 ArcEngine 一致）。
    let tmp = copy_fixture("es_noop");
    let gdb = Geodatabase::open(&tmp).unwrap();
    let session = gdb.edit_session();
    session.start();
    let fc = session.open_feature_class(&gdb, "地块").unwrap();
    fc.delete_feature(2).unwrap();
    assert_eq!(fc.feature_count(), 3);
    session.abort();
    assert_eq!(fc.feature_count(), 3, "无快照时不回滚（仅不写盘）");
    drop(session);
    drop(fc);
    drop(gdb);
    let gdb2 = Geodatabase::open(&tmp).unwrap();
    assert_eq!(oids_of(&gdb2, "地块"), vec![2, 3, 4, 5], "磁盘未变");
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------
// 数据表（TableHandle）路径 —— 用 sample 生成的 gdb 覆盖
// ---------------------------------------------------------------------------

#[test]
fn table_handle_crud_via_builder() {
    use gdb_core::builder::GeodatabaseBuilder;
    use gdb_core::field::{FieldDef, GeometryType, PrecisionGrid};

    let tmp = std::env::temp_dir().join("gdb_rs_crud_tbl");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let mut b = GeodatabaseBuilder::create(&tmp).unwrap();
    b.add_standalone_table(
        "T1",
        vec![
            FieldDef::new("Name", FieldType::String),
            FieldDef::new("Val", FieldType::Int32),
        ],
        vec![
            vec![FieldValue::ObjectId(1), FieldValue::Text("a".into()), FieldValue::Int32(1)],
            vec![FieldValue::ObjectId(2), FieldValue::Text("b".into()), FieldValue::Int32(2)],
            vec![FieldValue::ObjectId(3), FieldValue::Text("c".into()), FieldValue::Int32(3)],
        ],
    )
    .unwrap();
    let _ = GeometryType::Point;
    let _ = PrecisionGrid::default();

    let gdb = Geodatabase::open(&tmp).unwrap();
    let t = gdb.open_table("T1").unwrap();
    assert_eq!(t.row_count(), 3);
    // 按 where 更新
    let mut cur = t.update(&QueryFilter::where_clause("Val > 1").unwrap()).unwrap();
    let mut n = 0;
    while let Some(r) = cur.next_row() {
        r.set_by_name("Name", FieldValue::Text("x".into())).unwrap();
        cur.update(&r).unwrap();
        n += 1;
    }
    assert_eq!(n, 2);
    // 按 where 删除
    let d = t.delete_searched_rows(&QueryFilter::where_clause("Val = 3").unwrap()).unwrap();
    assert_eq!(d, 1);
    // 插入
    let oid = t
        .create_row(vec![
            FieldValue::ObjectId(0),
            FieldValue::Text("new".into()),
            FieldValue::Int32(9),
        ])
        .unwrap();
    t.save().unwrap();
    drop(t);
    drop(gdb);

    let gdb2 = Geodatabase::open(&tmp).unwrap();
    let t2 = gdb2.open_table("T1").unwrap();
    assert_eq!(t2.row_count(), 3, "3 - 1 + 1 = 3");
    let got = t2.query_oids(&QueryFilter::All).unwrap();
    assert!(got.contains(&oid), "新 OID 应存在: {got:?} vs {oid}");
    let _ = std::fs::remove_dir_all(&tmp);
}
