# gdb-rs — ESRI File Geodatabase (.gdb) 纯 Rust 解析与管理库

零外部依赖（仅 std）实现的 ESRI File Geodatabase 读写库，代码组织结构对齐
**ArcGIS ArcEngine** 的要素类遍历/更新模型：

```
Geodatabase（工作空间）
  ├─ FeatureClass / TableHandle / FeatureDataset
  │     └─ Cursor(Search|Update|Insert) ──> Row / Feature ──> store()
  └─ EditSession（start / start_operation / stop_operation / commit / abort）
```

## 能力

- **正确解析三类对象**：独立要素类、独立数据表、要素数据集中的要素类
- **按 OBJECTID 更新属性与几何**（点/多点/折线/面）
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
      feature_class.rs   FeatureClass / TableHandle：schema 与游标工厂
      feature_dataset.rs FeatureDataset：数据集容器
      cursor.rs          Cursor：Search / Update / Insert 遍历与更新
      row.rs / feature.rs Row / Feature：取值、赋值、store()
      edit.rs            EditSession：编辑会话与提交
      table.rs           .gdbtable / .gdbtablx 解析与回写
      geometry/mod.rs    几何编解码
      field.rs           字段类型、schema、精度网格
      value.rs           FieldValue、OLE 日期、UUID
      io.rs              小端读写、LEB128 varint、UTF-16
      catalog / xml / error / builder / lib
    tests/
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

# 逐行读取（要素类额外输出几何 WKT 摘要）
gdb read    /path/to/demo.gdb Cities_Tbl
gdb read    /path/to/demo.gdb Regions

# 按 OBJECTID 更新属性
gdb update-attr /path/to/demo.gdb Cities_Tbl 1 Name Beijing2

# 按 OBJECTID 更新点几何
gdb update-geom /path/to/demo.gdb Capitals 1 121.0 31.0
```

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

// 按 OBJECTID 更新属性 + 几何（编辑会话）
let session = gdb.edit_session();
session.start();
let fc = session.open_feature_class(&gdb, "Capitals")?;
let mut cur = fc.update(&QueryFilter::ByOid(1))?;
let f = cur.next_feature().unwrap();
f.set_by_name("Name", FieldValue::Text("NewCapital".into()))?;
f.set_geometry(Geometry::Point(Point { x: 121.0, y: 31.0 }))?;
f.store();
cur.update(f.row())?;
session.commit()?;
```

## 文件格式要点

`.gdbtable` 头部 40 字节（版本、要素数、字段描述段偏移 @32、文件大小 @24）。
行 blob：`[u32 row_len][u16 nOffsets][nOffsets×u16 偏移][null 位图][定长区][变长区]`，
偏移相对行起点。几何 blob：`[varuint geom_len][i32 geom_type][4 varuint 网格包络][坐标]`，
坐标以 zigzag 变长整数增量存储，真实坐标 = `i / xyscale + xorig`。

## 限制

- 几何主要为 2D 点/多点/线/面（Z/M 解析保留但在写入路径未启用）
- 针对 ArcGIS 生产的真实 `.gdb`（大偏移宽度 5/6 字节、UUID/XML 字段）为后续增强项
- 未与 GDAL 交叉校验（离线环境）
