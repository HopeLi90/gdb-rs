# gdb-rs — ESRI File Geodatabase (.gdb) 纯 Rust 解析与管理库

零外部依赖（仅 std）实现的 ESRI File Geodatabase 读写库，代码组织结构对齐
**ArcGIS ArcEngine** 的要素类遍历/更新模型：

```
Geodatabase（工作空间）
  ├─ FeatureClass / TableHandle / FeatureDataset
  │     └─ Cursor(Search|Update|Insert) ──> Row / Feature ──> store() / delete()
  ├─ QueryFilter（All | ByOid | ByOids | Where | Spatial | Combined）
  │     ├─ WhereClause（属性条件解析器）
  │     └─ SpatialFilter / SpatialRel（精确几何过滤）
  └─ EditSession（start / start_operation / stop_operation / commit / abort 回滚）
```

## 能力

- **正确解析三类对象**：独立要素类、独立数据表、要素数据集中的要素类
- **完整增删改（CRUD）**：
  - 新增：`create_feature` / `create_row`（自动分配 OBJECTID）
  - 查询：`QueryFilter` 支持 `All` / `ByOid` / `ByOids` / `Where`（属性条件）/ `Spatial`（精确空间过滤）/ `Combined`
  - 更新：按条件（属性/空间）批量更新，经 Update 游标 + `store()`
  - 删除：`delete_feature` / `delete_row` / `delete_searched_rows` / `delete_rows`（**保槽删除**，其余 OID 不变）
  - 编辑会话：`start_operation` / `stop_operation` 嵌套分组 + `abort()` 快照回滚
- `WhereClause` 手写解析器：支持 `= != > >= < <= AND OR NOT (...) IS [NOT] NULL [NOT] IN`、
  单引号字符串（含 `''` 转义与中文）、带引号字段名（`"Shape_Length"`）、数字字面量
- **精确空间过滤**（`ISpatialFilter` 风格）：`Intersects` / `Contains` / `Within` /
  `EnvelopeIntersects`；点在环内（射线法）、线段相交（含共线/端点）、面-面相交，
  洞按奇偶规则正确处理（非 bbox 近似）
- 解析 `a00000001` 系统目录（`GDB_SystemCatalog`），按 `Path` / `DatasetSubtype2` /
  `Definition` XML 自动分类对象类型
- `.gdbtable` 行 blob 编解码：定长/变长字段、null 位图、按字段顺序的偏移数组
- 几何 blob 编解码：精度网格量化 + zigzag 变长整数增量坐标（2D 点/线/面）

## 目录结构

```
crates/
  gdb_core/            核心库（纯 Rust，无依赖）
    src/
      workspace.rs       Geodatabase：打开 .gdb、列举/打开要素类·表·数据集
      catalog.rs         解析系统目录，分类独立表/独立要素类/数据集内要素类
      feature_class.rs   FeatureClass / TableHandle：schema、游标工厂、批量增删
      feature_dataset.rs FeatureDataset：数据集容器
      query_filter.rs    QueryFilter / WhereClause / SpatialFilter（过滤条件）
      cursor.rs          Cursor：Search / Update / Insert 遍历、更新、插入、删除
      row.rs / feature.rs Row / Feature：取值、赋值、store()、delete()
      edit.rs            EditSession：编辑会话、操作栈、提交与回滚
      table.rs           .gdbtable / .gdbtablx 解析、回写、行删除与快照
      geometry/
        mod.rs           几何编解码
        predicate.rs     精确空间谓词（相交/包含/被包含）
      field.rs           字段类型、schema、精度网格
      value.rs           FieldValue、OLE 日期、UUID
      io.rs              小端读写、LEB128 varint、UTF-16
      catalog / xml / error / builder / lib
    tests/
      real_gdb_test.rs   真实 .gdb 金标准测试（7 个）
      crud_test.rs       增删改集成测试（18 个，含保槽 OID 稳定性）
      roundtrip_test.rs  读写闭环集成测试（2 个）
  gdb_cli/             命令行工具（bin: gdb）
build-windows.sh       一键交叉编译 Windows x64 exe（Docker + USTC 镜像）
dist/gdb.exe           Windows 可执行程序（构建产物）
```

## 构建与测试

```bash
cargo build --workspace
cargo test  --workspace
```

## 构建 Windows 可执行程序（exe）

提供一键脚本，交叉编译出 Windows x64 命令行程序 **`dist/gdb.exe`**：

```bash
bash build-windows.sh
# ==> 已生成: dist/gdb.exe
```

产物验证：

```bash
file dist/gdb.exe
# PE32+ executable (console) x86-64, for MS Windows
```

该 exe **自包含**，仅依赖 Windows 系统 DLL（KERNEL32 / msvcrt / WS2_32 等），
可复制到任意 Windows 10/11 x64 直接运行，无需额外运行时或 DLL：

```powershell
.\gdb.exe --help
.\gdb.exe sample D:\data\demo.gdb
.\gdb.exe list   D:\data\demo.gdb
```

**工作原理**：脚本用 Docker（`rust:latest` 容器）+ USTC 镜像安装
`x86_64-pc-windows-gnu` 目标与 `gcc-mingw-w64-x86-64` 链接器后交叉编译，
适用于本机无法直连 `static.rust-lang.org` 的环境。可覆盖的环境变量：
`IMAGE`、`RUSTUP_DIST_SERVER`、`DEBIAN_MIRROR`。

**备选（本机为 Windows 且已装 Rust）**：

```powershell
cargo build --release -p gdb_cli
# 产物: target\release\gdb.exe
```

## 命令行用法

```bash
# 生成示例 .gdb（独立表 + 独立点要素类 + 要素数据集 + 数据集内面要素类）
gdb sample  /path/to/demo.gdb

# 列出所有对象（区分独立表 / 独立要素类 / 数据集内要素类）
gdb list    /path/to/demo.gdb

# 查看 schema 与几何类型
gdb describe /path/to/demo.gdb Capitals

# 逐行读取（可选 --where 条件；要素类额外输出几何 WKT 摘要）
gdb read    /path/to/demo.gdb Cities_Tbl
gdb read    /path/to/demo.gdb 地块 --where "Shape_Length > 300"

# 插入（要素类可用 --point / --ring 附带几何）
gdb insert /path/to/demo.gdb 地块 --set "BH=编号9" \
    --ring "40538389.49,3044658.394;40538499.68,3044695.035;40538449.01,3044778.332;40538389.49,3044742.125;40538389.49,3044658.394"

# 按条件批量更新字段（--set 可重复）
gdb update /path/to/demo.gdb 地块 --where "BH = '编号2'" --set "BH=编号2-改"

# 按条件/OBJECTID 批量删除（--dry-run 只预览命中数）
gdb delete /path/to/demo.gdb 地块 --where "BH = '编号1'" --dry-run
gdb delete /path/to/demo.gdb 地块 --where "BH = '编号1'"
gdb delete /path/to/demo.gdb 地块 --oids 3,5

# 兼容：按 OBJECTID 更新属性 / 点几何
gdb update-attr /path/to/demo.gdb Cities_Tbl 1 Name Beijing2
gdb update-geom /path/to/demo.gdb Capitals 1 121.0 31.0
```

> 注意 shell 引号：where 子句中的单引号需用双引号包裹整个参数，
> 例如 `--where "BH = '编号1'"`。

## 库 API 示例

```rust
use gdb_core::workspace::Geodatabase;
use gdb_core::cursor::QueryFilter;
use gdb_core::geometry::{Geometry, Point};
use gdb_core::value::FieldValue;

let gdb = Geodatabase::open("/path/to/demo.gdb".as_ref())?;
for fc in gdb.feature_classes() {
    println!("{} [{}]", fc.name, fc.geometry_type);
}

// 编辑会话 + 操作分组（可回滚）
let session = gdb.edit_session();
session.start();
session.start_operation()?;
let fc = session.open_feature_class(&gdb, "地块")?;

// 按属性条件批量更新
let filter = QueryFilter::where_clause("BH = '编号1' OR BH = '编号2'")?;
let mut cur = fc.update(&filter)?;
while let Some(f) = cur.next_feature() {
    f.set_by_name("BH", FieldValue::Text("已处理".into()))?;
    f.store();
    cur.update(f.row())?;
}

// 按条件批量删除
let n = fc.delete_searched_rows(&QueryFilter::where_clause("BH = '废弃'")?)?;
println!("删除 {n} 个要素");

// 新建要素（自动分配 OBJECTID）
let oid = fc.create_feature(vec![
    FieldValue::ObjectId(0),
    FieldValue::Geometry(Geometry::Point(Point { x: 116.4, y: 39.9 })),
    FieldValue::Text("新要素".into()),
])?;

session.stop_operation()?;
session.commit()?;   // 写盘；失败前可 session.abort() 回滚内存改动
```

## 文件格式要点

`.gdbtable` 头部 40 字节（版本、要素数、字段描述段偏移 @32、文件大小 @24）。
行 blob：`[u32 row_len][u16 nOffsets][nOffsets×u16 偏移][null 位图][定长区][变长区]`，
偏移相对行起点。几何 blob：`[varuint geom_len][i32 geom_type][4 varuint 网格包络][坐标]`，
坐标以 zigzag 变长整数增量存储，真实坐标 = `i / xyscale + xorig`。

## 限制

- 几何主要为 2D 点/多点/线/面（Z/M 解析保留但在写入路径未启用）
- **删除采用「保槽删除」**：被删行的槽位在 `.gdbtablx` 中置为 `offset=0`（空槽），
  其余要素 OBJECTID（= 槽位 + 1）保持不变，与 ArcGIS 语义一致。本实现**不写回**
  `aNNNNNNNN.freelist`，因此槽号不复用、文件不随删除缩小（仅影响空间效率，不影响正确性）。
- **空间过滤为精确几何判定**（点在环内 / 线段相交 / 面-面相交，洞按奇偶规则），
  但仅支持 XY 二维、不处理 Z/M 与曲线段几何；精度容差为绝对量级 `1e-9`。
- `abort()` 的内存回滚依赖 `start_operation()` 建立的快照；未开启操作时 `abort()`
  退化为「不写盘」（与 ArcEngine `AbortEditOperation` 语义一致）。
- **未与 GDAL 交叉校验**（离线环境）；金标准为真实 ArcGIS `.gdb` 的逐字节逆向 +
  行内 `Shape_Length`（几何周长）交叉验证。
