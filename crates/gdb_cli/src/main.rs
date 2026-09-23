//! `gdb` 命令行工具：演示并验证 `gdb_core` 对 ESRI File Geodatabase 的
//! 解析与管理能力（std-only，手工解析参数）。
//!
//! 子命令：
//!   gdb sample <gdb_dir>
//!       生成一个示例 .gdb（独立表 + 独立点要素类 + 要素数据集 + 数据集内面要素类）。
//!   gdb list <gdb_dir>
//!       列出目录中的独立表、独立要素类、要素数据集及其内要素类。
//!   gdb describe <gdb_dir> <name>
//!       显示指定对象（表/要素类）的字段定义与几何类型。
//!   gdb read <gdb_dir> <name>
//!       逐行输出对象内容（要素类额外输出几何 WKT 摘要）。
//!   gdb update-attr <gdb_dir> <name> <oid> <field> <value>
//!       按 OBJECTID 更新某字段（属性表/要素类通用）。
//!   gdb update-geom <gdb_dir> <name> <oid> <x> <y>
//!       按 OBJECTID 更新点要素几何（仅要素类）。

use std::process;

use gdb_core::builder::GeodatabaseBuilder;
use gdb_core::cursor::QueryFilter;
use gdb_core::field::{FieldDef, FieldType, GeometryType, PrecisionGrid};
use gdb_core::geometry::{Geometry, Point, Polygon};
use gdb_core::value::FieldValue;
use gdb_core::workspace::Geodatabase;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Err(e) = run(&args) {
        eprintln!("错误: {e}");
        process::exit(1);
    }
}

fn run(args: &[String]) -> Result<(), String> {
    if args.len() < 2 {
        print_help();
        return Err("缺少子命令".into());
    }
    let cmd = args[1].as_str();
    match cmd {
        "sample" => {
            let dir = expect_arg(args, 2, "gdb_dir")?;
            cmd_sample(&dir)
        }
        "list" => {
            let dir = expect_arg(args, 2, "gdb_dir")?;
            cmd_list(&dir)
        }
        "describe" => {
            let dir = expect_arg(args, 2, "gdb_dir")?;
            let name = expect_arg(args, 3, "name")?;
            cmd_describe(&dir, &name)
        }
        "read" => {
            let dir = expect_arg(args, 2, "gdb_dir")?;
            let name = expect_arg(args, 3, "name")?;
            cmd_read(&dir, &name)
        }
        "update-attr" => {
            let dir = expect_arg(args, 2, "gdb_dir")?;
            let name = expect_arg(args, 3, "name")?;
            let oid = expect_arg(args, 4, "oid")?.parse::<u64>().map_err(|_| "oid 必须为整数".to_string())?;
            let field = expect_arg(args, 5, "field")?;
            let value = expect_arg(args, 6, "value")?;
            cmd_update_attr(&dir, &name, oid, &field, &value)
        }
        "update-geom" => {
            let dir = expect_arg(args, 2, "gdb_dir")?;
            let name = expect_arg(args, 3, "name")?;
            let oid = expect_arg(args, 4, "oid")?.parse::<u64>().map_err(|_| "oid 必须为整数".to_string())?;
            let x = expect_arg(args, 5, "x")?.parse::<f64>().map_err(|_| "x 必须为浮点数".to_string())?;
            let y = expect_arg(args, 6, "y")?.parse::<f64>().map_err(|_| "y 必须为浮点数".to_string())?;
            cmd_update_geom(&dir, &name, oid, x, y)
        }
        "-h" | "--help" | "help" => {
            print_help();
            Ok(())
        }
        other => {
            print_help();
            Err(format!("未知子命令: {other}"))
        }
    }
}

fn expect_arg<'a>(args: &'a [String], idx: usize, name: &str) -> Result<&'a str, String> {
    args.get(idx)
        .map(|s| s.as_str())
        .ok_or_else(|| format!("缺少参数 <{name}>"))
}

fn print_help() {
    println!(
        "用法:\n\
         gdb sample <gdb_dir>\n\
         gdb list <gdb_dir>\n\
         gdb describe <gdb_dir> <name>\n\
         gdb read <gdb_dir> <name>\n\
         gdb update-attr <gdb_dir> <name> <oid> <field> <value>\n\
         gdb update-geom <gdb_dir> <name> <oid> <x> <y>"
    );
}

/// 生成示例 .gdb（覆盖三类对象：独立表 / 独立要素类 / 数据集内要素类）。
fn cmd_sample(dir: &str) -> Result<(), String> {
    let path = std::path::Path::new(dir);
    if path.join("a00000001.gdbtable").exists() {
        return Err(format!("{dir} 已存在 .gdb 内容，请换用空目录"));
    }
    let grid = PrecisionGrid::default();
    let mut b = GeodatabaseBuilder::create(path).map_err(|e| e.to_string())?;

    // 1) 独立数据表
    b.add_standalone_table(
        "Cities_Tbl",
        vec![
            FieldDef::new("Name", FieldType::String),
            FieldDef::new("Pop", FieldType::Int32),
        ],
        vec![
            vec![FieldValue::ObjectId(1), FieldValue::Text("Beijing".into()), FieldValue::Int32(2154)],
            vec![FieldValue::ObjectId(2), FieldValue::Text("Shanghai".into()), FieldValue::Int32(2424)],
        ],
    )
    .map_err(|e| e.to_string())?;

    // 2) 独立点要素类
    let mut gf = FieldDef::new("SHAPE", FieldType::Geometry);
    gf.grid = grid;
    gf.srs_wkt = "GEOGCS[\"GCS_WGS_1984\"]".into();
    gf.nullable = true;
    b.add_standalone_feature_class(
        "Capitals",
        GeometryType::Point,
        gf.clone(),
        vec![FieldDef::new("Name", FieldType::String)],
        vec![vec![
            FieldValue::ObjectId(1),
            FieldValue::Geometry(Geometry::Point(Point { x: 116.4, y: 39.9 })),
            FieldValue::Text("Beijing".into()),
        ]],
    )
    .map_err(|e| e.to_string())?;

    // 3) 要素数据集 + 数据集内面要素类
    b.add_feature_dataset("Admin").map_err(|e| e.to_string())?;
    b.add_feature_class_in_dataset(
        "Admin",
        "Regions",
        GeometryType::Polygon,
        gf,
        vec![FieldDef::new("Name", FieldType::String)],
        vec![vec![
            FieldValue::ObjectId(1),
            FieldValue::Geometry(Geometry::Polygon(Polygon {
                rings: vec![vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)]],
            })),
            FieldValue::Text("RegionA".into()),
        ]],
    )
    .map_err(|e| e.to_string())?;

    println!("已生成示例 .gdb: {dir}");
    Ok(())
}

/// 将字段值格式化为可读字符串（几何字段显示 WKT 摘要而非内部表示）。
fn show_value(v: &FieldValue) -> String {
    match v {
        FieldValue::Int16(x) => x.to_string(),
        FieldValue::Int32(x) => x.to_string(),
        FieldValue::Int64(x) => x.to_string(),
        FieldValue::Float(x) => x.to_string(),
        FieldValue::Double(x) => x.to_string(),
        FieldValue::Text(s) => s.clone(),
        FieldValue::Xml(s) => s.clone(),
        FieldValue::DateTime(dt) => dt.to_string_iso(),
        FieldValue::ObjectId(x) => x.to_string(),
        FieldValue::Geometry(g) => geom_to_wkt(g),
        FieldValue::Binary(b) => format!("<binary {} bytes>", b.len()),
        FieldValue::Uuid(u) => u.to_string(),
        FieldValue::Null => "<null>".to_string(),
    }
}

/// 几何摘要（WKT 风格）。
fn geom_to_wkt(g: &Geometry) -> String {
    match g {
        Geometry::Point(p) => format!("POINT({} {})", p.x, p.y),
        Geometry::MultiPoint(mp) => {
            let pts: Vec<String> = mp
                .points
                .iter()
                .map(|(x, y)| format!("({x} {y})"))
                .collect();
            format!("MULTIPOINT({})", pts.join(","))
        }
        Geometry::Polyline(pl) => {
            let total: usize = pl.parts.iter().map(|p| p.len()).sum();
            format!("POLYLINE({} parts, {} points)", pl.parts.len(), total)
        }
        Geometry::Polygon(pg) => {
            let total: usize = pg.rings.iter().map(|r| r.len()).sum();
            format!("POLYGON({} rings, {} points)", pg.rings.len(), total)
        }
    }
}

fn open_gdb(dir: &str) -> Result<Geodatabase, String> {
    Geodatabase::open(std::path::Path::new(dir)).map_err(|e| e.to_string())
}

fn cmd_list(dir: &str) -> Result<(), String> {
    let gdb = open_gdb(dir)?;
    let tables = gdb.tables();
    let fcs = gdb.feature_classes();
    let fds = gdb.feature_datasets();

    println!("独立数据表 ({}):", tables.len());
    for t in &tables {
        let rows = gdb
            .open_table(&t.name)
            .map(|h| h.row_count().to_string())
            .unwrap_or_else(|_| "?".into());
        println!("  - {}  (rows={})", t.name, rows);
    }
    println!("独立要素类 ({}):", fcs.len());
    for f in &fcs {
        if f.parent_dataset.is_none() {
            let n = gdb
                .open_feature_class(&f.name)
                .map(|c| c.feature_count().to_string())
                .unwrap_or_else(|_| "?".into());
            println!("  - {}  [{}]  (features={})", f.name, geom_name(f.geometry_type), n);
        }
    }
    println!("要素数据集 ({}):", fds.len());
    for d in &fds {
        println!("  - {} (要素数据集)", d.name);
    }
    println!("要素数据集中的要素类:");
    let mut any = false;
    for f in &fcs {
        if let Some(parent) = &f.parent_dataset {
            any = true;
            let n = gdb
                .open_feature_class(&f.name)
                .map(|c| c.feature_count().to_string())
                .unwrap_or_else(|_| "?".into());
            println!(
                "  - {}  [{}]  (所属: {}, features={})",
                f.name,
                geom_name(f.geometry_type),
                parent,
                n
            );
        }
    }
    if !any {
        println!("  (无)");
    }
    Ok(())
}

fn geom_name(g: GeometryType) -> &'static str {
    match g {
        GeometryType::Point => "Point",
        GeometryType::MultiPoint => "MultiPoint",
        GeometryType::Polyline => "Polyline",
        GeometryType::Polygon => "Polygon",
        GeometryType::Envelope => "Envelope",
        GeometryType::MultiPatch => "MultiPatch",
        GeometryType::None => "None",
        GeometryType::Other(c) => return Box::leak(format!("Other({c})").into_boxed_str()),
    }
}

fn cmd_describe(dir: &str, name: &str) -> Result<(), String> {
    let gdb = open_gdb(dir)?;
    // 先尝试要素类，再尝试表。
    if let Ok(fc) = gdb.open_feature_class(name) {
        println!("要素类: {}", fc.name());
        println!("几何类型: {}", geom_name(fc.shape_type()));
        if !fc.spatial_reference().is_empty() {
            println!("空间参考: {}", fc.spatial_reference());
        }
        describe_fields(&fc.fields());
        return Ok(());
    }
    let tbl = gdb.open_table(name).map_err(|e| e.to_string())?;
    println!("数据表: {}", tbl.name());
    describe_fields(&tbl.fields());
    Ok(())
}

fn describe_fields(fields: &[gdb_core::field::FieldDef]) {
    println!("字段 ({}):", fields.len());
    println!("  {:>3}  {:<20} {:<10} {}", "idx", "name", "type", "nullable");
    for (i, f) in fields.iter().enumerate() {
        println!(
            "  {:>3}  {:<20} {:<10} {}",
            i,
            f.name,
            type_name(f.field_type),
            if f.nullable { "yes" } else { "no" }
        );
    }
}

fn type_name(t: FieldType) -> &'static str {
    match t {
        FieldType::Int16 => "int16",
        FieldType::Int32 => "int32",
        FieldType::Float32 => "float32",
        FieldType::Float64 => "float64",
        FieldType::String => "string",
        FieldType::DateTime => "datetime",
        FieldType::ObjectId => "oid",
        FieldType::Geometry => "geometry",
        FieldType::Binary => "binary",
        FieldType::Raster => "raster",
        FieldType::Uuid => "uuid",
        FieldType::Xml => "xml",
        FieldType::Int64 => "int64",
        FieldType::Date => "date",
        FieldType::Time => "time",
        FieldType::Other(c) => return Box::leak(format!("other({c})").into_boxed_str()),
    }
}

fn cmd_read(dir: &str, name: &str) -> Result<(), String> {
    let gdb = open_gdb(dir)?;
    if let Ok(fc) = gdb.open_feature_class(name) {
        let mut cur = fc.search(&QueryFilter::All).map_err(|e| e.to_string())?;
        println!("要素类 {} ({} 要素):", fc.name(), fc.feature_count());
        while let Some(f) = cur.next_feature() {
            let oid = f.object_id();
            let mut parts = Vec::new();
            for fd in fc.fields() {
                if fd.field_type == FieldType::Geometry {
                    match f.geometry() {
                        Ok(g) => parts.push(format!("SHAPE={}", geom_to_wkt(&g))),
                        Err(_) => parts.push("SHAPE=<null>".to_string()),
                    }
                } else {
                    match f.get_by_name(&fd.name) {
                        Ok(v) => parts.push(format!("{}={}", fd.name, show_value(&v))),
                        Err(_) => parts.push(format!("{}=<err>", fd.name)),
                    }
                }
            }
            println!("  OID {} | {}", oid, parts.join(" | "));
        }
        return Ok(());
    }
    let tbl = gdb.open_table(name).map_err(|e| e.to_string())?;
    let mut cur = tbl.search(&QueryFilter::All).map_err(|e| e.to_string())?;
    println!("数据表 {} ({} 行):", tbl.name(), tbl.row_count());
    while let Some(r) = cur.next_row() {
        let mut parts = Vec::new();
        for fd in tbl.fields() {
            match r.get_by_name(&fd.name) {
                Ok(v) => parts.push(format!("{}={}", fd.name, show_value(&v))),
                Err(_) => parts.push(format!("{}=<err>", fd.name)),
            }
        }
        println!("  {}", parts.join(" | "));
    }
    Ok(())
}

/// 按 OBJECTID 更新属性（表/要素类通用）。优先按要素类打开以演示几何对象亦可更新。
fn cmd_update_attr(dir: &str, name: &str, oid: u64, field: &str, value: &str) -> Result<(), String> {
    let gdb = open_gdb(dir)?;
    let session = gdb.edit_session();
    session.start();

    // 定位字段类型：要素类与表都用 field_index 查询 schema。
    let ftype = if let Ok(fc) = gdb.open_feature_class(name) {
        fc.fields()
            .iter()
            .find(|f| f.name == field)
            .map(|f| f.field_type)
    } else {
        gdb.open_table(name)
            .ok()
            .and_then(|t| t.fields().iter().find(|f| f.name == field).map(|f| f.field_type))
    }
    .ok_or_else(|| format!("对象 {name} 中找不到字段 {field}"))?;

    let parsed = parse_value_for_type(value, ftype)?;

    // 在编辑会话内打开并完成更新。
    if let Ok(fc) = session.open_feature_class(&gdb, name) {
        let mut cur = fc.update(&QueryFilter::ByOid(oid)).map_err(|e| e.to_string())?;
        if let Some(f) = cur.next_feature() {
            f.set_by_name(field, parsed).map_err(|e| e.to_string())?;
            f.store();
            cur.update(f.row()).map_err(|e| e.to_string())?;
        } else {
            return Err(format!("要素类 {name} 中不存在 OBJECTID={oid}"));
        }
    } else {
        let t = session.open_table(&gdb, name).map_err(|e| e.to_string())?;
        let mut cur = t.update(&QueryFilter::ByOid(oid)).map_err(|e| e.to_string())?;
        if let Some(r) = cur.next_row() {
            r.set_by_name(field, parsed).map_err(|e| e.to_string())?;
            r.store();
            cur.update(&r).map_err(|e| e.to_string())?;
        } else {
            return Err(format!("表 {name} 中不存在 OBJECTID={oid}"));
        }
    }
    session.commit().map_err(|e| e.to_string())?;
    println!("已更新 {name} OBJECTID={oid} 字段 {field} = {value}");
    Ok(())
}

fn cmd_update_geom(dir: &str, name: &str, oid: u64, x: f64, y: f64) -> Result<(), String> {
    let gdb = open_gdb(dir)?;
    let session = gdb.edit_session();
    session.start();
    let fc = session.open_feature_class(&gdb, name).map_err(|e| e.to_string())?;
    let mut cur = fc.update(&QueryFilter::ByOid(oid)).map_err(|e| e.to_string())?;
    let f = cur
        .next_feature()
        .ok_or_else(|| format!("要素类 {name} 中不存在 OBJECTID={oid}"))?;
    f.set_geometry(Geometry::Point(Point { x, y }))
        .map_err(|e| e.to_string())?;
    f.store();
    cur.update(f.row()).map_err(|e| e.to_string())?;
    session.commit().map_err(|e| e.to_string())?;
    println!("已更新 {name} OBJECTID={oid} 几何 = POINT({x} {y})");
    Ok(())
}

/// 将字符串按目标字段类型解析为 FieldValue。
fn parse_value_for_type(s: &str, ft: FieldType) -> Result<FieldValue, String> {
    match ft {
        FieldType::Int16 => s.parse::<i16>().map(FieldValue::Int16).map_err(|_| "无法解析为 int16".into()),
        FieldType::Int32 => s.parse::<i32>().map(FieldValue::Int32).map_err(|_| "无法解析为 int32".into()),
        FieldType::Float32 => s.parse::<f32>().map(FieldValue::Float).map_err(|_| "无法解析为 float32".into()),
        FieldType::Float64 => s.parse::<f64>().map(FieldValue::Double).map_err(|_| "无法解析为 float64".into()),
        FieldType::String => Ok(FieldValue::Text(s.to_string())),
        FieldType::Xml => Ok(FieldValue::Xml(s.to_string())),
        FieldType::DateTime => Ok(FieldValue::Text(s.to_string())), // 简化：以文本暂存
        other => Err(format!("CLI 暂不支持写入 {} 类型字段", type_name(other))),
    }
}
