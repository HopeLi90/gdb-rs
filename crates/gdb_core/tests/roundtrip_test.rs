//! 读写闭环测试：用 builder 生成最小 .gdb（独立表 + 独立点要素类 + 要素数据集内面要素类），
//! 再用 Geodatabase 打开读取，校验目录枚举、行读取、几何读取与更新回写。

use gdb_core::builder::GeodatabaseBuilder;
use gdb_core::field::{FieldDef, FieldType, GeometryType, PrecisionGrid};
use gdb_core::geometry::{Geometry, Point, Polygon};
use gdb_core::value::FieldValue;
use gdb_core::workspace::Geodatabase;
use gdb_core::cursor::QueryFilter;
use std::path::PathBuf;

fn sample_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("gdb_rs_sample_test");
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn build(dir: &PathBuf) {
    let grid = PrecisionGrid::default();
    let mut b = GeodatabaseBuilder::create(dir).unwrap();

    // 独立数据表
    let trows = vec![
        vec![
            FieldValue::ObjectId(1),
            FieldValue::Text("Beijing".into()),
            FieldValue::Int32(2154),
        ],
        vec![
            FieldValue::ObjectId(2),
            FieldValue::Text("Shanghai".into()),
            FieldValue::Int32(2424),
        ],
    ];
    b.add_standalone_table(
        "Cities_Tbl",
        vec![
            FieldDef::new("Name", FieldType::String),
            FieldDef::new("Pop", FieldType::Int32),
        ],
        trows,
    )
    .unwrap();

    // 独立点要素类
    let mut gf = FieldDef::new("SHAPE", FieldType::Geometry);
    gf.grid = grid;
    gf.srs_wkt = "GEOGCS[\"GCS_WGS_1984\"]".into();
    gf.nullable = true;
    let prows = vec![vec![
        FieldValue::ObjectId(1),
        FieldValue::Geometry(Geometry::Point(Point { x: 116.4, y: 39.9 })),
        FieldValue::Text("Capital".into()),
    ]];
    b.add_standalone_feature_class(
        "Capitals",
        GeometryType::Point,
        gf.clone(),
        vec![FieldDef::new("Name", FieldType::String)],
        prows,
    )
    .unwrap();

    // 要素数据集 + 面要素类
    b.add_feature_dataset("Admin").unwrap();
    let fdpoly = vec![vec![
        FieldValue::ObjectId(1),
        FieldValue::Geometry(Geometry::Polygon(Polygon {
            rings: vec![vec![
                (0.0, 0.0),
                (0.0, 1.0),
                (1.0, 1.0),
                (1.0, 0.0),
                (0.0, 0.0),
            ]],
        })),
        FieldValue::Text("RegionA".into()),
    ]];
    b.add_feature_class_in_dataset(
        "Admin",
        "Regions",
        GeometryType::Polygon,
        gf,
        vec![FieldDef::new("Name", FieldType::String)],
        fdpoly,
    )
    .unwrap();
}

#[test]
fn enumerate_and_read() {
    let dir = sample_dir();
    build(&dir);

    let gdb = Geodatabase::open(&dir).unwrap();
    assert_eq!(gdb.tables().len(), 1, "独立表数量");
    assert_eq!(gdb.feature_classes().len(), 2, "要素类数量(独立 + 数据集内)");
    assert_eq!(gdb.feature_datasets().len(), 1, "要素数据集数量");

    // 独立表读取
    let tbl = gdb.open_table("Cities_Tbl").unwrap();
    assert_eq!(tbl.row_count(), 2);
    let mut cur = tbl.search(&QueryFilter::All).unwrap();
    let r = cur.next_row().unwrap();
    assert_eq!(r.get_by_name("Name").unwrap(), FieldValue::Text("Beijing".into()));
    assert_eq!(r.get_by_name("Pop").unwrap(), FieldValue::Int32(2154));

    // 独立点要素类几何读取
    let fc = gdb.open_feature_class("Capitals").unwrap();
    assert_eq!(fc.shape_type(), GeometryType::Point);
    assert_eq!(fc.feature_count(), 1);
    let mut cur = fc.search(&QueryFilter::All).unwrap();
    let f = cur.next_feature().unwrap();
    match f.geometry().unwrap() {
        Geometry::Point(p) => {
            assert!((p.x - 116.4).abs() < 1e-6);
            assert!((p.y - 39.9).abs() < 1e-6);
        }
        other => panic!("期望 Point，得到 {other:?}"),
    }

    // 要素数据集内面要素类
    let fd = gdb.open_feature_dataset("Admin").unwrap();
    let fcs = fd.feature_classes(&gdb).unwrap();
    assert_eq!(fcs.len(), 1);
    assert_eq!(fcs[0].name(), "Regions");
    let poly = gdb.open_feature_class("Regions").unwrap();
    let mut cur = poly.search(&QueryFilter::All).unwrap();
    let f = cur.next_feature().unwrap();
    match f.geometry().unwrap() {
        Geometry::Polygon(pg) => assert_eq!(pg.rings.len(), 1),
        other => panic!("期望 Polygon，得到 {other:?}"),
    }
}

#[test]
fn update_geometry_and_attribute() {
    let dir = sample_dir();
    build(&dir);

    let gdb = Geodatabase::open(&dir).unwrap();
    let fc = gdb.open_feature_class("Capitals").unwrap();

    // 通过编辑会话按 OBJECTID 更新几何与属性
    let session = gdb.edit_session();
    session.start();
    let fc_s = session.open_feature_class(&gdb, "Capitals").unwrap();
    let mut cur = fc_s.update(&QueryFilter::ByOid(1)).unwrap();
    let f = cur.next_feature().unwrap();
    f.set_by_name("Name", FieldValue::Text("NewCapital".into())).unwrap();
    f.set_geometry(Geometry::Point(Point { x: 121.0, y: 31.0 }))
        .unwrap();
    f.store();
    session.commit().unwrap();
    drop(session);

    // 重新打开，校验更新生效
    let gdb2 = Geodatabase::open(&dir).unwrap();
    let fc2 = gdb2.open_feature_class("Capitals").unwrap();
    let mut cur = fc2.search(&QueryFilter::All).unwrap();
    let f = cur.next_feature().unwrap();
    assert_eq!(f.get_by_name("Name").unwrap(), FieldValue::Text("NewCapital".into()));
    match f.geometry().unwrap() {
        Geometry::Point(p) => {
            assert!((p.x - 121.0).abs() < 1e-6);
            assert!((p.y - 31.0).abs() < 1e-6);
        }
        other => panic!("更新后几何错误: {other:?}"),
    }
    let _ = fc;
}
