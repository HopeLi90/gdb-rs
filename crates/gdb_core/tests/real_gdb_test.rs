//! 真实 ArcGIS .gdb 金标准测试（fixture: tests/fixtures/test.gdb）。
//!
//! 全部断言值均来自逐字节逆向分析 + GDAL OpenFileGDB 对照 + 行内
//! `Shape_Length`（几何周长）金标准交叉验证，任何解析偏差都会在此暴露。

use gdb_core::catalog::{enumerate, CatalogItemType};
use gdb_core::cursor::QueryFilter;
use gdb_core::field::{FieldType, GeometryType};
use gdb_core::geometry::Geometry;
use gdb_core::table::Table;
use gdb_core::value::FieldValue;
use gdb_core::workspace::Geodatabase;
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test.gdb")
}

/// 计算几何总长度（周长：面/线各部件累计折线长）。
fn geometry_length(g: &Geometry) -> f64 {
    let pts: Vec<Vec<(f64, f64)>> = match g {
        Geometry::Polygon(p) => p.rings.clone(),
        Geometry::Polyline(p) => p.parts.clone(),
        Geometry::Point(_) => return 0.0,
        Geometry::MultiPoint(_) => return 0.0,
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

#[test]
fn system_catalog_and_items() {
    let dir = fixture();
    let items = enumerate(&dir).unwrap();
    assert_eq!(items.len(), 2, "应有 2 个用户对象");
    assert!(
        items
            .iter()
            .all(|i| i.item_type == CatalogItemType::FeatureClass),
        "均为独立要素类"
    );
    let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
    assert!(names.contains(&"地块"));
    assert!(names.contains(&"fw_DealData"));
    // 几何类型码（DatasetSubtype2=4 → Polygon）
    let dk = items.iter().find(|i| i.name == "地块").unwrap();
    assert_eq!(dk.geometry_type, GeometryType::Polygon);
    assert_eq!(dk.file_id, 9, "地块 → a00000009");
    let fw = items.iter().find(|i| i.name == "fw_DealData").unwrap();
    assert_eq!(fw.file_id, 0xA, "fw_DealData → a0000000a");
}

#[test]
fn user_feature_class_dikuai() {
    let dir = fixture();
    let t = Table::open(&dir, 9).unwrap();
    // 字段清单与类型
    let names: Vec<&str> = t.schema.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["OBJECTID", "Shape", "SZ", "BH", "Shape_Length", "Shape_Area"]);
    assert_eq!(t.schema.fields[2].alias, "四至坐标", "中文别名");
    assert_eq!(t.rows.len(), 4, "4 个有效要素（5 槽位含 1 空槽）");

    // OID 由槽位合成：槽 0 为空，有效行在槽 1..5 → OID=2..5
    let oi = t.schema.objectid_index().unwrap();
    for (i, row) in t.rows.iter().enumerate() {
        assert!(
            matches!(row[oi], FieldValue::ObjectId(v) if v == (i + 2) as u64),
            "第 {i} 行 OID 应为 {}",
            i + 2
        );
    }

    // 中文字段值（第 1 行）
    let sz = t.schema.field_index("SZ").unwrap();
    let bh = t.schema.field_index("BH").unwrap();
    assert_eq!(t.rows[0][bh], FieldValue::Text("编号1".into()));
    let sz0 = match &t.rows[0][sz] {
        FieldValue::Text(s) => s.clone(),
        other => panic!("SZ 应为文本，得到 {other:?}"),
    };
    assert!(sz0.starts_with("东至:(40538499.680,3044695.035)"), "SZ={sz0}");

    // 金标准：解码几何周长 == 存储 Shape_Length（6 位小数精确）
    let gi = t.schema.geometry_index().unwrap();
    let sl = t.schema.field_index("Shape_Length").unwrap();
    for row in &t.rows {
        let geom = match &row[gi] {
            FieldValue::Geometry(g) => g,
            other => panic!("Shape 应为几何，得到 {other:?}"),
        };
        let expected = match &row[sl] {
            FieldValue::Double(v) => *v,
            other => panic!("Shape_Length 应为 double，得到 {other:?}"),
        };
        let got = geometry_length(geom);
        assert!(
            (got - expected).abs() < 1e-3,
            "周长 {got} 与存储 Shape_Length {expected} 偏差过大"
        );
    }
    // 精确抽查第 1 行（金标准值）
    let sl0 = match &t.rows[0][sl] {
        FieldValue::Double(v) => *v,
        _ => unreachable!(),
    };
    assert!((sl0 - 335.175337).abs() < 5e-7, "金标准 Shape_Length={sl0}");
}

#[test]
fn user_feature_class_fwdealdata() {
    let dir = fixture();
    let t = Table::open(&dir, 0xA).unwrap();
    assert_eq!(t.schema.fields.len(), 40, "40 个字段");
    assert_eq!(t.rows.len(), 6151, "6151 个有效要素");
    // Shape 字段不可空（flags=0x06）而 Shape_Length 可空（flags=0x03）
    let shape = &t.schema.fields[1];
    assert_eq!(shape.name, "Shape");
    assert!(!shape.nullable, "fw_DealData.Shape 不可空");
    let sl = t.schema.field_index("Shape_Length").unwrap();
    assert!(t.schema.fields[sl].nullable, "Shape_Length 可空");

    // 几何周长金标准（全部 6151 行）
    let gi = t.schema.geometry_index().unwrap();
    let mut checked = 0usize;
    for row in &t.rows {
        if let (FieldValue::Geometry(g), FieldValue::Double(expected)) = (&row[gi], &row[sl]) {
            let got = geometry_length(g);
            assert!(
                (got - expected).abs() < 1e-3,
                "行周长 {got} != Shape_Length {expected}"
            );
            checked += 1;
        }
    }
    assert!(checked > 6000, "应校验大量几何周长，实际 {checked}");

    // 中文字段抽样（首行）
    let prov = t.schema.field_index("province").unwrap();
    assert_eq!(t.rows[0][prov], FieldValue::Text("山西省".into()));
}

#[test]
fn workspace_level_api() {
    let dir = fixture();
    let gdb = Geodatabase::open(&dir).unwrap();
    assert_eq!(gdb.feature_classes().len(), 2);
    assert_eq!(gdb.tables().len(), 0);
    assert_eq!(gdb.feature_datasets().len(), 0);

    let fc = gdb.open_feature_class("地块").unwrap();
    assert_eq!(fc.feature_count(), 4);
    assert_eq!(fc.shape_type(), GeometryType::Polygon);

    // ArcEngine 风格游标读取
    let mut cur = fc.search(&QueryFilter::All).unwrap();
    let mut n = 0;
    while cur.next_feature().is_some() {
        n += 1;
    }
    assert_eq!(n, 4);

    // fw_DealData 6151 要素
    let fw = gdb.open_feature_class("fw_DealData").unwrap();
    assert_eq!(fw.feature_count(), 6151);
}

#[test]
fn update_on_real_copy() {
    // 在临时副本上做属性+几何更新并回读校验（不动 fixture）。
    let tmp = std::env::temp_dir().join("gdb_rs_real_update_test");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    for entry in std::fs::read_dir(fixture()).unwrap() {
        let entry = entry.unwrap();
        let dst = tmp.join(entry.file_name());
        std::fs::copy(entry.path(), dst).unwrap();
    }

    let gdb = Geodatabase::open(&tmp).unwrap();
    let session = gdb.edit_session();
    session.start();
    let fc = session.open_feature_class(&gdb, "地块").unwrap();
    // 注意：槽 0 为空，OID 从 2 开始
    let mut cur = fc.update(&QueryFilter::ByOid(2)).unwrap();
    let f = cur.next_feature().expect("OID=2 应存在");
    f.set_by_name("BH", FieldValue::Text("编号1-改".into())).unwrap();
    f.set_geometry(Geometry::Polygon(gdb_core::geometry::Polygon {
        rings: vec![vec![
            (40538389.490, 3044658.394),
            (40538499.680, 3044695.035),
            (40538449.010, 3044778.332),
            (40538389.490, 3044742.125),
            (40538389.490, 3044658.394),
        ]],
    }))
    .unwrap();
    f.store();
    session.commit().unwrap();
    drop(session);

    // 重新打开校验
    let gdb2 = Geodatabase::open(&tmp).unwrap();
    let fc2 = gdb2.open_feature_class("地块").unwrap();
    let mut cur = fc2.search(&QueryFilter::ByOid(2)).unwrap();
    let f = cur.next_feature().expect("更新后 OID=2 应存在");
    assert_eq!(f.get_by_name("BH").unwrap(), FieldValue::Text("编号1-改".into()));
    match f.geometry().unwrap() {
        Geometry::Polygon(pg) => {
            assert_eq!(pg.rings.len(), 1);
            assert_eq!(pg.rings[0].len(), 5);
            assert!((pg.rings[0][0].0 - 40538389.490).abs() < 1e-3);
        }
        other => panic!("更新后应为面几何: {other:?}"),
    }
    // 其余 3 行不受影响
    assert_eq!(fc2.feature_count(), 4);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn all_system_tables_open() {
    // 除 GDB_ReplicaLog（a00000008 文件不存在）外全部可打开。
    let dir = fixture();
    let t = Table::open(&dir, 1).unwrap();
    assert_eq!(t.rows.len(), 10, "SystemCatalog 10 行");
    let name_i = t.schema.field_index("Name").unwrap();
    let names: Vec<String> = t
        .rows
        .iter()
        .map(|r| match &r[name_i] {
            FieldValue::Text(s) => s.clone(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(names[0], "GDB_SystemCatalog");
    assert_eq!(names[3], "GDB_Items");
    assert_eq!(names[8], "地块");
    assert_eq!(names[9], "fw_DealData");

    // GDB_Items 打开 + 4 行（根/Workspace/两个用户对象）
    let items = Table::open(&dir, 4).unwrap();
    assert_eq!(items.rows.len(), 4);
    assert_eq!(items.schema.fields.len(), 17);

    // GDB_ItemTypes 打开 + 34 行
    let types = Table::open(&dir, 5).unwrap();
    assert_eq!(types.rows.len(), 34);

    // a00000008 缺失 → 报 IO 错误而非 panic
    assert!(Table::open(&dir, 8).is_err());
}

#[test]
fn field_type_codes() {
    let dir = fixture();
    let t = Table::open(&dir, 9).unwrap();
    assert_eq!(t.schema.fields[0].field_type, FieldType::ObjectId);
    assert_eq!(t.schema.fields[1].field_type, FieldType::Geometry);
    assert_eq!(t.schema.fields[2].field_type, FieldType::String);
    assert_eq!(t.schema.fields[4].field_type, FieldType::Float64);
    // 几何字段网格参数（来自真实文件：xyscale=10000，空间索引网格 5900）
    let g = &t.schema.fields[1].grid;
    assert!((g.xyscale - 10000.0).abs() < 1e-9);
    assert_eq!(g.grid_sizes.len(), 1);
    assert!((g.grid_sizes[0] - 5900.0).abs() < 1e-9);
}
