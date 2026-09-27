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
//!   gdb read <gdb_dir> <name> [--where <sql>]
//!       逐行输出对象内容（要素类额外输出几何 WKT 摘要）。
//!   gdb insert <gdb_dir> <name> [--set F=V]... [--point x,y | --ring "x1,y1;x2,y2;..."]
//!       向表/要素类插入一行（要素类可附带几何），打印新 OBJECTID。
//!   gdb update <gdb_dir> <name> --set F=V [--set F=V]... (--oid N | --where <sql>)
//!       按 OBJECTID 或 where 条件批量更新字段。
//!   gdb delete <gdb_dir> <name> (--oid N | --oids N,N,N | --where <sql>) [--dry-run]
//!       按 OBJECTID 集合或 where 条件批量删除（对应 ITable.DeleteSearchedRows）。
//!   gdb update-attr <gdb_dir> <name> <oid> <field> <value>
//!       按 OBJECTID 更新某字段（属性表/要素类通用，保留兼容）。
//!   gdb update-geom <gdb_dir> <name> <oid> <x> <y>
//!       按 OBJECTID 更新点要素几何（仅要素类，保留兼容）。

use std::process;

use gdb_core::builder::GeodatabaseBuilder;
use gdb_core::cursor::QueryFilter;
use gdb_core::field::{FieldDef, FieldType, GeometryType, PrecisionGrid};
use gdb_core::geometry::{Geometry, Point, Polygon};
use gdb_core::value::{DateTime, FieldValue, Uuid};
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
            cmd_read(&dir, &name, args.get(4..).unwrap_or(&[]))
        }
        "insert" => {
            let dir = expect_arg(args, 2, "gdb_dir")?;
            let name = expect_arg(args, 3, "name")?;
            cmd_insert(&dir, &name, args.get(4..).unwrap_or(&[]))
        }
        "update" => {
            let dir = expect_arg(args, 2, "gdb_dir")?;
            let name = expect_arg(args, 3, "name")?;
            cmd_update(&dir, &name, args.get(4..).unwrap_or(&[]))
        }
        "delete" => {
            let dir = expect_arg(args, 2, "gdb_dir")?;
            let name = expect_arg(args, 3, "name")?;
            cmd_delete(&dir, &name, args.get(4..).unwrap_or(&[]))
        }
        "update-attr" => {
            let dir = expect_arg(args, 2, "gdb_dir")?;
            let name = expect_arg(args, 3, "name")?;
            let oid = expect_arg(args, 4, "oid")?
                .parse::<u64>()
                .map_err(|_| "oid 必须为整数".to_string())?;
            let field = expect_arg(args, 5, "field")?;
            let value = expect_arg(args, 6, "value")?;
            cmd_update_attr(&dir, &name, oid, &field, &value)
        }
        "update-geom" => {
            let dir = expect_arg(args, 2, "gdb_dir")?;
            let name = expect_arg(args, 3, "name")?;
            let oid = expect_arg(args, 4, "oid")?
                .parse::<u64>()
                .map_err(|_| "oid 必须为整数".to_string())?;
            let x = expect_arg(args, 5, "x")?
                .parse::<f64>()
                .map_err(|_| "x 必须为浮点数".to_string())?;
            let y = expect_arg(args, 6, "y")?
                .parse::<f64>()
                .map_err(|_| "y 必须为浮点数".to_string())?;
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
         gdb read <gdb_dir> <name> [--where <sql>]\n\
         gdb insert <gdb_dir> <name> [--set F=V]... [--point x,y | --ring \"x1,y1;x2,y2;...\"]\n\
         gdb update <gdb_dir> <name> --set F=V [--set F=V]... (--oid N | --where <sql>)\n\
         gdb delete <gdb_dir> <name> (--oid N | --oids N,N,N | --where <sql>) [--dry-run]\n\
         gdb update-attr <gdb_dir> <name> <oid> <field> <value>\n\
         gdb update-geom <gdb_dir> <name> <oid> <x> <y>\n\
         \n\
         示例:\n\
         gdb delete demo.gdb 地块 --where \"BH = '编号1'\"\n\
         gdb update demo.gdb 地块 --where \"BH = '编号2'\" --set \"BH=编号2-改\"\n\
         gdb insert demo.gdb 地块 --set \"BH=编号9\" --ring \"0,0;10,0;10,10;0,10;0,0\""
    );
}

/// 极简命令行选项解析（std-only）。
///
/// 支持 `--key value`、`--key=value` 与可重复选项（`--set`）；`--flag` 形式
/// 视为无值开关。
struct FlagParser {
    opts: Vec<(String, Option<String>)>,
}

impl FlagParser {
    fn parse(args: &[String]) -> Self {
        let mut opts: Vec<(String, Option<String>)> = Vec::new();
        let mut i = 0usize;
        while i < args.len() {
            let a = args[i].as_str();
            if let Some(rest) = a.strip_prefix("--") {
                if let Some((k, v)) = rest.split_once('=') {
                    opts.push((k.to_string(), Some(v.to_string())));
                } else {
                    // 看下一个是否作为值（不以 -- 开头）。
                    if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                        opts.push((rest.to_string(), Some(args[i + 1].clone())));
                        i += 1;
                    } else {
                        opts.push((rest.to_string(), None));
                    }
                }
            }
            i += 1;
        }
        FlagParser { opts }
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.opts
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .and_then(|(_, v)| v.as_deref())
    }

    fn get_all(&self, name: &str) -> Vec<&str> {
        self.opts
            .iter()
            .filter(|(k, _)| k == name)
            .filter_map(|(_, v)| v.as_deref())
            .collect()
    }

    fn has(&self, name: &str) -> bool {
        self.opts.iter().any(|(k, _)| k == name)
    }
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

// ---------------------------------------------------------------------------
// sample
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// list / describe
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// 目标对象（表 / 要素类）统一封装
// ---------------------------------------------------------------------------

/// 从 where / oid / oids 选项构造查询条件（三者互斥）。
fn filter_from_flags(flags: &FlagParser) -> Result<QueryFilter, String> {
    let where_sql = flags.get("where");
    let oid = flags.get("oid");
    let oids = flags.get("oids");
    let provided = [where_sql.is_some(), oid.is_some(), oids.is_some()]
        .iter()
        .filter(|b| **b)
        .count();
    if provided == 0 {
        return Err("必须指定 --oid、--oids 或 --where 之一".into());
    }
    if provided > 1 {
        return Err("--oid、--oids、--where 只能指定其一".into());
    }
    if let Some(s) = where_sql {
        return QueryFilter::where_clause(s).map_err(|e| e.to_string());
    }
    if let Some(s) = oid {
        let v = s.parse::<u64>().map_err(|_| "oid 必须为整数".to_string())?;
        return Ok(QueryFilter::ByOid(v));
    }
    let s = oids.unwrap();
    let mut list = Vec::new();
    for part in s.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        list.push(p.parse::<u64>().map_err(|_| format!("oids 含非法整数: {p}"))?);
    }
    Ok(QueryFilter::ByOids(list))
}

/// 打开目标对象（优先要素类，其次数据表），返回字段定义列表。
fn target_fields(gdb: &Geodatabase, name: &str) -> Result<Vec<FieldDef>, String> {
    if let Ok(fc) = gdb.open_feature_class(name) {
        return Ok(fc.fields());
    }
    gdb.open_table(name)
        .map(|t| t.fields())
        .map_err(|_| format!("找不到对象 {name}（既非要素类也非数据表）"))
}

fn cmd_read(dir: &str, name: &str, rest: &[String]) -> Result<(), String> {
    let flags = FlagParser::parse(rest);
    let filter = match flags.get("where") {
        Some(sql) => QueryFilter::where_clause(sql).map_err(|e| e.to_string())?,
        None => QueryFilter::All,
    };
    let gdb = open_gdb(dir)?;
    if let Ok(fc) = gdb.open_feature_class(name) {
        let mut cur = fc.search(&filter).map_err(|e| e.to_string())?;
        println!("要素类 {} (共 {} 要素，条件 {filter}):", fc.name(), fc.feature_count());
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
    let mut cur = tbl.search(&filter).map_err(|e| e.to_string())?;
    println!("数据表 {} (共 {} 行，条件 {filter}):", tbl.name(), tbl.row_count());
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

// ---------------------------------------------------------------------------
// update
// ---------------------------------------------------------------------------

/// 按 OBJECTID 或 where 条件批量更新字段（对应 ArcEngine 更新游标 + `IRow.Store`）。
fn cmd_update(dir: &str, name: &str, rest: &[String]) -> Result<(), String> {
    let flags = FlagParser::parse(rest);
    let sets = flags.get_all("set");
    if sets.is_empty() {
        return Err("update 至少需要一个 --set F=V".into());
    }
    let filter = filter_from_flags(&flags)?;
    let fields = target_fields(&open_gdb(dir)?, name)?;

    // 预先按字段类型解析所有赋值。
    let mut assignments: Vec<(String, FieldValue)> = Vec::new();
    for s in &sets {
        let (fname, raw) = s
            .split_once('=')
            .ok_or_else(|| format!("--set 参数应为 F=V 形式: {s}"))?;
        let fd = fields
            .iter()
            .find(|f| f.name == fname)
            .ok_or_else(|| format!("对象 {name} 中找不到字段 {fname}"))?;
        let val = parse_value_for_type(raw, fd.field_type)?;
        assignments.push((fname.to_string(), val));
    }

    let gdb = open_gdb(dir)?;
    let session = gdb.edit_session();
    session.start();
    session.start_operation().map_err(|e| e.to_string())?;

    let count = if let Ok(fc) = session.open_feature_class(&gdb, name) {
        let mut cur = fc.update(&filter).map_err(|e| e.to_string())?;
        let mut n = 0usize;
        while let Some(f) = cur.next_feature() {
            for (k, v) in &assignments {
                f.set_by_name(k, v.clone()).map_err(|e| e.to_string())?;
            }
            f.store();
            cur.update(f.row()).map_err(|e| e.to_string())?;
            n += 1;
        }
        n
    } else {
        let t = session.open_table(&gdb, name).map_err(|e| e.to_string())?;
        let mut cur = t.update(&filter).map_err(|e| e.to_string())?;
        let mut n = 0usize;
        while let Some(r) = cur.next_row() {
            for (k, v) in &assignments {
                r.set_by_name(k, v.clone()).map_err(|e| e.to_string())?;
            }
            r.store();
            cur.update(&r).map_err(|e| e.to_string())?;
            n += 1;
        }
        n
    };

    session.stop_operation().map_err(|e| e.to_string())?;
    session.commit().map_err(|e| e.to_string())?;
    println!("已更新 {name} 共 {count} 行（条件 {filter}）");
    Ok(())
}

// ---------------------------------------------------------------------------
// delete
// ---------------------------------------------------------------------------

/// 按 OBJECTID 集合或 where 条件批量删除（对应 `ITable.DeleteSearchedRows`）。
fn cmd_delete(dir: &str, name: &str, rest: &[String]) -> Result<(), String> {
    let flags = FlagParser::parse(rest);
    let dry_run = flags.has("dry-run");
    let filter = filter_from_flags(&flags)?;

    let gdb = open_gdb(dir)?;
    if dry_run {
        let n = if let Ok(fc) = gdb.open_feature_class(name) {
            fc.query_oids(&filter).map_err(|e| e.to_string())?.len()
        } else {
            gdb.open_table(name)
                .map_err(|e| e.to_string())?
                .query_oids(&filter)
                .map_err(|e| e.to_string())?
                .len()
        };
        println!("[dry-run] {name} 匹配 {n} 行（条件 {filter}），未做改动");
        return Ok(());
    }

    let session = gdb.edit_session();
    session.start();
    session.start_operation().map_err(|e| e.to_string())?;

    let count = if let Ok(fc) = session.open_feature_class(&gdb, name) {
        fc.delete_searched_rows(&filter).map_err(|e| e.to_string())?
    } else {
        session
            .open_table(&gdb, name)
            .map_err(|e| e.to_string())?
            .delete_searched_rows(&filter)
            .map_err(|e| e.to_string())?
    };

    session.stop_operation().map_err(|e| e.to_string())?;
    session.commit().map_err(|e| e.to_string())?;
    println!("已从 {name} 删除 {count} 行（条件 {filter}）");
    Ok(())
}

// ---------------------------------------------------------------------------
// insert
// ---------------------------------------------------------------------------

/// 向表/要素类插入一行（对应 `IFeatureClass.CreateFeature` / `ITable.CreateRow`）。
fn cmd_insert(dir: &str, name: &str, rest: &[String]) -> Result<(), String> {
    let flags = FlagParser::parse(rest);
    let gdb = open_gdb(dir)?;
    let fields = target_fields(&gdb, name)?;

    // 解析 --set F=V
    let mut provided: Vec<(String, FieldValue)> = Vec::new();
    for s in flags.get_all("set") {
        let (fname, raw) = s
            .split_once('=')
            .ok_or_else(|| format!("--set 参数应为 F=V 形式: {s}"))?;
        let fd = fields
            .iter()
            .find(|f| f.name == fname)
            .ok_or_else(|| format!("对象 {name} 中找不到字段 {fname}"))?;
        let val = parse_value_for_type(raw, fd.field_type)?;
        provided.push((fname.to_string(), val));
    }

    // 解析几何
    let geom = parse_geometry_flags(&flags)?;

    // 组装整行值：已提供优先，其次字段默认值，最后按可空/类型零值。
    let values: Vec<FieldValue> = fields
        .iter()
        .map(|fd| {
            if let Some((_, v)) = provided.iter().find(|(k, _)| *k == fd.name) {
                return v.clone();
            }
            if fd.field_type == FieldType::Geometry {
                if let Some(g) = &geom {
                    return FieldValue::Geometry(g.clone());
                }
            }
            if fd.field_type == FieldType::ObjectId {
                return FieldValue::ObjectId(0); // 占位，写入时覆盖
            }
            if let Some(def) = &fd.default {
                return def.clone();
            }
            if fd.nullable {
                FieldValue::Null
            } else {
                zero_value(fd.field_type)
            }
        })
        .collect();

    let session = gdb.edit_session();
    session.start();
    session.start_operation().map_err(|e| e.to_string())?;

    let new_oid = if let Ok(fc) = session.open_feature_class(&gdb, name) {
        fc.create_feature(values).map_err(|e| e.to_string())?
    } else {
        session
            .open_table(&gdb, name)
            .map_err(|e| e.to_string())?
            .create_row(values)
            .map_err(|e| e.to_string())?
    };

    session.stop_operation().map_err(|e| e.to_string())?;
    session.commit().map_err(|e| e.to_string())?;
    println!("已向 {name} 插入 1 行，新 OBJECTID = {new_oid}");
    Ok(())
}

/// 解析 `--point x,y` 或 `--ring "x1,y1;x2,y2;..."` 为几何。
fn parse_geometry_flags(flags: &FlagParser) -> Result<Option<Geometry>, String> {
    if let Some(p) = flags.get("point") {
        let (x, y) = parse_xy(p)?;
        return Ok(Some(Geometry::Point(Point { x, y })));
    }
    if let Some(r) = flags.get("ring") {
        let mut pts = Vec::new();
        for seg in r.split(';') {
            let seg = seg.trim();
            if seg.is_empty() {
                continue;
            }
            pts.push(parse_xy(seg)?);
        }
        if pts.len() < 3 {
            return Err("--ring 至少需要 3 个点".into());
        }
        return Ok(Some(Geometry::Polygon(Polygon { rings: vec![pts] })));
    }
    Ok(None)
}

fn parse_xy(s: &str) -> Result<(f64, f64), String> {
    let (a, b) = s
        .split_once(',')
        .ok_or_else(|| format!("坐标应为 x,y 形式: {s}"))?;
    let x = a.trim().parse::<f64>().map_err(|_| format!("x 非法: {a}"))?;
    let y = b.trim().parse::<f64>().map_err(|_| format!("y 非法: {b}"))?;
    Ok((x, y))
}

/// 按字段类型给出零值（非空字段未提供值时的兜底）。
fn zero_value(t: FieldType) -> FieldValue {
    match t {
        FieldType::Int16 => FieldValue::Int16(0),
        FieldType::Int32 => FieldValue::Int32(0),
        FieldType::Int64 => FieldValue::Int64(0),
        FieldType::Float32 => FieldValue::Float(0.0),
        FieldType::Float64 => FieldValue::Double(0.0),
        FieldType::String => FieldValue::Text(String::new()),
        FieldType::Xml => FieldValue::Xml(String::new()),
        FieldType::DateTime | FieldType::Date | FieldType::Time => {
            FieldValue::DateTime(DateTime::from_ole(0.0))
        }
        FieldType::Uuid => FieldValue::Uuid(Uuid::from_bytes([0u8; 16])),
        FieldType::Binary | FieldType::Raster => FieldValue::Binary(Vec::new()),
        FieldType::Geometry => FieldValue::Null,
        FieldType::ObjectId => FieldValue::ObjectId(0),
        FieldType::Other(_) => FieldValue::Null,
    }
}

// ---------------------------------------------------------------------------
// 保留的按 OID 更新命令（兼容旧用法）
// ---------------------------------------------------------------------------

/// 按 OBJECTID 更新属性（表/要素类通用）。
fn cmd_update_attr(dir: &str, name: &str, oid: u64, field: &str, value: &str) -> Result<(), String> {
    cmd_update(
        dir,
        name,
        &[
            "--oid".to_string(),
            oid.to_string(),
            "--set".to_string(),
            format!("{field}={value}"),
        ],
    )
}

/// 按 OBJECTID 更新点要素几何。
fn cmd_update_geom(dir: &str, name: &str, oid: u64, x: f64, y: f64) -> Result<(), String> {
    let gdb = open_gdb(dir)?;
    let session = gdb.edit_session();
    session.start();
    session.start_operation().map_err(|e| e.to_string())?;
    let fc = session.open_feature_class(&gdb, name).map_err(|e| e.to_string())?;
    let mut cur = fc.update(&QueryFilter::ByOid(oid)).map_err(|e| e.to_string())?;
    let f = cur
        .next_feature()
        .ok_or_else(|| format!("要素类 {name} 中不存在 OBJECTID={oid}"))?;
    f.set_geometry(Geometry::Point(Point { x, y }))
        .map_err(|e| e.to_string())?;
    f.store();
    cur.update(f.row()).map_err(|e| e.to_string())?;
    session.stop_operation().map_err(|e| e.to_string())?;
    session.commit().map_err(|e| e.to_string())?;
    println!("已更新 {name} OBJECTID={oid} 几何 = POINT({x} {y})");
    Ok(())
}

// ---------------------------------------------------------------------------
// 值解析
// ---------------------------------------------------------------------------

/// 将字符串按目标字段类型解析为 FieldValue。
fn parse_value_for_type(s: &str, ft: FieldType) -> Result<FieldValue, String> {
    // 统一支持字面量 null（大小写不敏感）。
    if s.eq_ignore_ascii_case("null") {
        return Ok(FieldValue::Null);
    }
    match ft {
        FieldType::Int16 => s.parse::<i16>().map(FieldValue::Int16).map_err(|_| "无法解析为 int16".into()),
        FieldType::Int32 => s.parse::<i32>().map(FieldValue::Int32).map_err(|_| "无法解析为 int32".into()),
        FieldType::Int64 => s.parse::<i64>().map(FieldValue::Int64).map_err(|_| "无法解析为 int64".into()),
        FieldType::Float32 => s.parse::<f32>().map(FieldValue::Float).map_err(|_| "无法解析为 float32".into()),
        FieldType::Float64 => s.parse::<f64>().map(FieldValue::Double).map_err(|_| "无法解析为 float64".into()),
        FieldType::ObjectId => s.parse::<u64>().map(FieldValue::ObjectId).map_err(|_| "无法解析为 OID".into()),
        FieldType::String => Ok(FieldValue::Text(s.to_string())),
        FieldType::Xml => Ok(FieldValue::Xml(s.to_string())),
        FieldType::DateTime | FieldType::Date | FieldType::Time => parse_datetime(s),
        FieldType::Uuid => parse_uuid(s),
        other => Err(format!("CLI 暂不支持写入 {} 类型字段", type_name(other))),
    }
}

/// 解析日期时间：接受 `YYYY-MM-DD`、`YYYY-MM-DD HH:MM:SS`、`YYYY-MM-DDTHH:MM:SS`，
/// 可选小数秒 `.ffffff`。
fn parse_datetime(s: &str) -> Result<FieldValue, String> {
    let (date_part, time_part) = match s.split_once(['T', ' ']) {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let mut d = date_part.split('-');
    let y: i32 = d
        .next()
        .and_then(|x| x.parse().ok())
        .ok_or_else(|| format!("日期非法（应形如 YYYY-MM-DD）: {s}"))?;
    let mo: u32 = d
        .next()
        .and_then(|x| x.parse().ok())
        .ok_or_else(|| format!("日期缺少月份: {s}"))?;
    let da: u32 = d
        .next()
        .and_then(|x| x.parse().ok())
        .ok_or_else(|| format!("日期缺少日: {s}"))?;
    let (mut hh, mut mi, mut ss, mut micros) = (0u32, 0u32, 0u32, 0u32);
    if let Some(t) = time_part {
        let mut tp = t.split(':');
        hh = tp.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        mi = tp.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        if let Some(sec) = tp.next() {
            let (whole, f) = match sec.split_once('.') {
                Some((w, fr)) => (w, fr),
                None => (sec, ""),
            };
            ss = whole.parse().unwrap_or(0);
            // 小数秒 → 微秒（左侧补零到 6 位）。
            if !f.is_empty() {
                let mut digits: String = f.chars().filter(|c| c.is_ascii_digit()).collect();
                digits.truncate(6);
                while digits.len() < 6 {
                    digits.push('0');
                }
                micros = digits.parse().unwrap_or(0);
            }
        }
    }
    Ok(FieldValue::DateTime(DateTime {
        year: y,
        month: mo,
        day: da,
        hour: hh,
        minute: mi,
        second: ss,
        micros,
    }))
}

/// 解析 UUID：剥离非十六进制字符，要求恰为 32 位十六进制数字。
fn parse_uuid(s: &str) -> Result<FieldValue, String> {
    let hex: Vec<u8> = s.chars().filter(|c| c.is_ascii_hexdigit()).map(|c| c as u8).collect();
    if hex.len() != 32 {
        return Err(format!(
            "UUID 应为 32 位十六进制字符（当前 {} 位）: {s}",
            hex.len()
        ));
    }
    let mut arr = [0u8; 16];
    for k in 0..16 {
        let hi = (hex[k * 2] as char).to_digit(16).unwrap_or(0) as u8;
        let lo = (hex[k * 2 + 1] as char).to_digit(16).unwrap_or(0) as u8;
        arr[k] = hi << 4 | lo;
    }
    Ok(FieldValue::Uuid(Uuid::from_bytes(arr)))
}
