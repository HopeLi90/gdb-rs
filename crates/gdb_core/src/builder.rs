//! 创建（写入）File Geodatabase 的构建器：用于生成最小 .gdb 夹具与演示。
//!
//! 目录结构与真实 ESRI .gdb 一致（两级目录，已对照真实数据实证）：
//! - `a00000001` = GDB_SystemCatalog：`ID/Name/FileFormat`，文件号 ↔ 内部表名映射；
//! - `a00000002` = GDB_Items：用户对象注册表（Type GUID + Definition XML 分类）；
//! - `a00000003` = GDB_ItemTypes：`UUID/ParentTypeID/Name` 类型字典；
//! - `a00000004+` = 用户数据文件。
//!
//! ItemTypes 的 GUID 使用 ESRI 公开的标准常量，生成的 .gdb 可被本 crate 的
//! `Geodatabase::open`/`catalog::enumerate` 正确枚举。

use std::path::{Path, PathBuf};

use crate::error::{GdbError, Result};
use crate::field::{FieldDef, FieldType, GeometryType, TableSchema};
use crate::table::Table;
use crate::value::{FieldValue, Uuid};

// ESRI 标准 ItemTypes GUID（16 字节原始存储，与真实 GDB 数据一致）。
const TYPE_FOLDER: [u8; 16] = [
    0x6f, 0x3e, 0x78, 0xf3, 0xca, 0x65, 0x14, 0x45, 0x83, 0x15, 0xce, 0x39, 0x85, 0xda, 0xd3, 0xb1,
];
const TYPE_WORKSPACE: [u8; 16] = [
    0x0f, 0xfe, 0x73, 0xc6, 0x80, 0x72, 0x4f, 0x40, 0x85, 0x32, 0x20, 0x75, 0x5d, 0xd8, 0xfc, 0x06,
];
const TYPE_FEATURE_CLASS: [u8; 16] = [
    0x09, 0x78, 0x73, 0x70, 0x2c, 0x85, 0x03, 0x4a, 0x9e, 0x22, 0x2c, 0xce, 0xea, 0x5b, 0x9b, 0xfa,
];
const TYPE_TABLE: [u8; 16] = [
    0x3b, 0xbc, 0x06, 0xcd, 0x9d, 0x78, 0x51, 0x4c, 0xaa, 0xfa, 0xa4, 0x67, 0x91, 0x2b, 0x89, 0x65,
];
const TYPE_FEATURE_DATASET: [u8; 16] = [
    0x49, 0x71, 0x73, 0x74, 0xb5, 0xdc, 0x57, 0x42, 0x89, 0x04, 0xb9, 0x72, 0x4e, 0x32, 0xa5, 0x30,
];

/// 生成确定性合成 UUID（每行 GlobalId；自读自写一致即可）。
fn synthetic_uuid(n: u64) -> Uuid {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&n.to_le_bytes());
    b[8] = 0x42;
    b[15] = 0x1b;
    Uuid(b)
}

fn oid_field() -> FieldDef {
    let mut f = FieldDef::new("OBJECTID", FieldType::ObjectId);
    f.nullable = false;
    f.required = true;
    f.editable = false;
    f
}

/// GDB_SystemCatalog（a00000001）schema：ID/Name/FileFormat。
fn system_catalog_schema() -> TableSchema {
    let mut id = FieldDef::new("ID", FieldType::Int32);
    id.nullable = false;
    id.required = true;
    id.editable = false;
    let mut name = FieldDef::new("Name", FieldType::String);
    name.nullable = false;
    name.editable = true;
    let mut fmt = FieldDef::new("FileFormat", FieldType::Int32);
    fmt.nullable = false;
    fmt.editable = true;
    TableSchema {
        geometry_type: GeometryType::None,
        has_z: false,
        has_m: false,
        string_utf8: true,
        fields: vec![oid_field(), id, name, fmt],
    }
}

/// GDB_Items（用户对象注册表）schema（17 字段，与真实布局一致）。
fn items_schema() -> TableSchema {
    let mut uuid = FieldDef::new("UUID", FieldType::Uuid);
    uuid.nullable = false;
    uuid.required = true;
    uuid.editable = false;
    let mut itype = FieldDef::new("Type", FieldType::Uuid);
    itype.nullable = false;
    itype.editable = true;
    let s = FieldDef::new;
    TableSchema {
        geometry_type: GeometryType::Polygon,
        has_z: false,
        has_m: false,
        string_utf8: true,
        fields: vec![
            oid_field(),
            uuid,
            itype,
            s("Name", FieldType::String),
            s("PhysicalName", FieldType::String),
            s("Path", FieldType::String),
            s("DatasetSubtype1", FieldType::Int32),
            s("DatasetSubtype2", FieldType::Int32),
            s("DatasetInfo1", FieldType::String),
            s("DatasetInfo2", FieldType::String),
            s("URL", FieldType::String),
            s("Definition", FieldType::Xml),
            s("Documentation", FieldType::Xml),
            s("ItemInfo", FieldType::Xml),
            s("Properties", FieldType::Int32),
            s("Defaults", FieldType::Binary),
            s("Shape", FieldType::Geometry),
        ],
    }
}

/// GDB_ItemTypes（类型字典）schema：UUID/ParentTypeID/Name。
fn item_types_schema() -> TableSchema {
    let s = FieldDef::new;
    TableSchema {
        geometry_type: GeometryType::None,
        has_z: false,
        has_m: false,
        string_utf8: true,
        fields: vec![
            oid_field(),
            s("UUID", FieldType::Uuid),
            s("ParentTypeID", FieldType::Uuid),
            s("Name", FieldType::String),
        ],
    }
}

/// 创建 .gdb 的构建器。
pub struct GeodatabaseBuilder {
    dir: PathBuf,
    next_file_id: u32,
    next_uuid: u64,
    /// GDB_SystemCatalog 行（ID/Name/FileFormat）。
    catalog_rows: Vec<Vec<FieldValue>>,
    /// GDB_Items 注册行。
    items_rows: Vec<Vec<FieldValue>>,
}

impl GeodatabaseBuilder {
    /// 在 `dir` 处新建一个空 .gdb（写入系统目录、GDB_Items 与 GDB_ItemTypes）。
    pub fn create(dir: &Path) -> Result<GeodatabaseBuilder> {
        std::fs::create_dir_all(dir)?;
        let mut b = GeodatabaseBuilder {
            dir: dir.to_path_buf(),
            next_file_id: 4,
            next_uuid: 0x100,
            catalog_rows: Vec::new(),
            items_rows: Vec::new(),
        };
        // 系统表自身也登记在 SystemCatalog（与真实 GDB 一致）。
        for (id, name) in [(1, "GDB_SystemCatalog"), (2, "GDB_Items"), (3, "GDB_ItemTypes")] {
            b.catalog_rows.push(vec![
                FieldValue::ObjectId(id),
                FieldValue::Int32(id as i32),
                FieldValue::Text(name.to_string()),
                FieldValue::Int32(1),
            ]);
        }
        // 根（Folder）与工作空间占位行。
        b.push_item(TYPE_FOLDER, "", 0, 0, "");
        b.push_item(TYPE_WORKSPACE, "Workspace", 0, 0, "<DEWorkspace/>");
        b.flush_catalog()?;
        b.flush_items()?;
        b.flush_item_types()?;
        Ok(b)
    }

    fn flush_catalog(&self) -> Result<()> {
        let mut t = Table::new(&self.dir, 1, 3, system_catalog_schema());
        for row in &self.catalog_rows {
            t.add_row(row.clone())?;
        }
        t.flush()
    }

    fn flush_items(&self) -> Result<()> {
        let mut t = Table::new(&self.dir, 2, 3, items_schema());
        for row in &self.items_rows {
            t.add_row(row.clone())?;
        }
        t.flush()
    }

    fn flush_item_types(&self) -> Result<()> {
        let mut t = Table::new(&self.dir, 3, 3, item_types_schema());
        // 标准类型字典（ParentTypeID=Dataset(899eda28…) 对核心类型无实际影响，
        // 读取端仅用 UUID→Name 映射，这里统一填 0）。
        let entries: [(&[u8; 16], &str); 5] = [
            (&TYPE_FOLDER, "Folder"),
            (&TYPE_WORKSPACE, "Workspace"),
            (&TYPE_FEATURE_CLASS, "Feature Class"),
            (&TYPE_TABLE, "Table"),
            (&TYPE_FEATURE_DATASET, "Feature Dataset"),
        ];
        for (i, (guid, name)) in entries.iter().enumerate() {
            t.add_row(vec![
                FieldValue::ObjectId((i + 1) as u64),
                FieldValue::Uuid(Uuid(**guid)),
                FieldValue::Uuid(Uuid([0u8; 16])),
                FieldValue::Text(name.to_string()),
            ])?;
        }
        t.flush()
    }

    /// 追加一行 GDB_Items 注册记录（Path 字段可在调用后覆写）。
    fn push_item(
        &mut self,
        type_guid: [u8; 16],
        name: &str,
        subtype1: i32,
        subtype2: i32,
        definition: &str,
    ) {
        self.next_uuid += 1;
        let uuid = synthetic_uuid(self.next_uuid);
        let path = format!("\\{name}");
        self.items_rows.push(vec![
            FieldValue::ObjectId(0), // 由槽位合成，占位
            FieldValue::Uuid(uuid),
            FieldValue::Uuid(Uuid(type_guid)),
            FieldValue::Text(name.to_string()),
            FieldValue::Text(name.to_uppercase()),
            FieldValue::Text(path),
            FieldValue::Int32(subtype1),
            if subtype2 != 0 {
                FieldValue::Int32(subtype2)
            } else {
                FieldValue::Null
            },
            FieldValue::Text(if subtype2 != 0 { "SHAPE".into() } else { String::new() }),
            FieldValue::Null,
            FieldValue::Text(String::new()),
            FieldValue::Xml(definition.to_string()),
            FieldValue::Null,
            FieldValue::Null,
            FieldValue::Int32(1),
            FieldValue::Null,
            FieldValue::Null,
        ]);
    }

    /// 添加一个数据对象（表/要素类）：写数据文件 + 更新两级目录。
    fn add_item(
        &mut self,
        type_guid: [u8; 16],
        name: &str,
        subtype1: i32,
        subtype2: i32,
        definition: &str,
        path: &str,
        schema: TableSchema,
        rows: Vec<Vec<FieldValue>>,
    ) -> Result<u32> {
        let file_id = self.next_file_id;
        self.next_file_id += 1;

        let mut table = Table::new(&self.dir, file_id, 3, schema);
        for row in rows {
            table.add_row(row)?;
        }
        table.flush()?;

        // SystemCatalog：文件号 ↔ 内部表名。
        self.catalog_rows.push(vec![
            FieldValue::ObjectId(file_id as u64),
            FieldValue::Int32(file_id as i32),
            FieldValue::Text(name.to_string()),
            FieldValue::Int32(1),
        ]);

        // GDB_Items 注册行（path 覆盖默认 \name；Path 字段索引 5）。
        self.push_item(type_guid, name, subtype1, subtype2, definition);
        if let Some(item) = self.items_rows.last_mut() {
            item[5] = FieldValue::Text(path.to_string());
        }
        self.flush_catalog()?;
        self.flush_items()?;
        Ok(file_id)
    }

    /// 添加独立数据表。
    pub fn add_standalone_table(
        &mut self,
        name: &str,
        fields: Vec<FieldDef>,
        rows: Vec<Vec<FieldValue>>,
    ) -> Result<u32> {
        let mut schema = Table::empty_schema();
        schema.fields.extend(fields);
        let definition = format!("<DETable><Name>{name}</Name></DETable>");
        let path = format!("\\{name}");
        self.add_item(
            TYPE_TABLE,
            name,
            2,
            0,
            &definition,
            &path,
            schema,
            rows,
        )
    }

    /// 添加独立要素类。
    pub fn add_standalone_feature_class(
        &mut self,
        name: &str,
        geometry_type: GeometryType,
        mut geometry_field: FieldDef,
        fields: Vec<FieldDef>,
        rows: Vec<Vec<FieldValue>>,
    ) -> Result<u32> {
        geometry_field.field_type = FieldType::Geometry;
        let mut schema = Table::empty_schema();
        schema.geometry_type = geometry_type;
        schema.fields.push(geometry_field);
        schema.fields.extend(fields);
        let gname = match geometry_type {
            GeometryType::Point => "esriGeometryPoint",
            GeometryType::MultiPoint => "esriGeometryMultipoint",
            GeometryType::Polyline => "esriGeometryPolyline",
            GeometryType::Polygon => "esriGeometryPolygon",
            other => return Err(GdbError::GeometryError(format!("暂不支持的几何类型 {other:?}"))),
        };
        let srs = &schema.fields[1].srs_wkt;
        let definition = format!(
            "<DEFeatureClassInfo><Name>{name}</Name><GeometryType>{gname}</GeometryType><SpatialReference><WKT>{srs}</WKT></SpatialReference></DEFeatureClassInfo>"
        );
        let path = format!("\\{name}");
        self.add_item(
            TYPE_FEATURE_CLASS,
            name,
            1,
            geometry_type.code(),
            &definition,
            &path,
            schema,
            rows,
        )
    }

    /// 添加要素数据集（虚拟容器，无数据文件、不占文件号）。
    pub fn add_feature_dataset(&mut self, name: &str) -> Result<u32> {
        let definition = format!("<DEFeatureDataset><Name>{name}</Name></DEFeatureDataset>");
        self.push_item(TYPE_FEATURE_DATASET, name, 0, 0, &definition);
        self.flush_items()?;
        // 返回 0 表示无独立数据文件。
        Ok(0)
    }

    /// 在指定要素数据集中添加要素类。
    pub fn add_feature_class_in_dataset(
        &mut self,
        dataset: &str,
        name: &str,
        geometry_type: GeometryType,
        mut geometry_field: FieldDef,
        fields: Vec<FieldDef>,
        rows: Vec<Vec<FieldValue>>,
    ) -> Result<u32> {
        geometry_field.field_type = FieldType::Geometry;
        let mut schema = Table::empty_schema();
        schema.geometry_type = geometry_type;
        schema.fields.push(geometry_field);
        schema.fields.extend(fields);
        let gname = match geometry_type {
            GeometryType::Point => "esriGeometryPoint",
            GeometryType::MultiPoint => "esriGeometryMultipoint",
            GeometryType::Polyline => "esriGeometryPolyline",
            GeometryType::Polygon => "esriGeometryPolygon",
            other => return Err(GdbError::GeometryError(format!("暂不支持的几何类型 {other:?}"))),
        };
        let srs = &schema.fields[1].srs_wkt;
        let definition = format!(
            "<DEFeatureClassInfo><Name>{name}</Name><GeometryType>{gname}</GeometryType><SpatialReference><WKT>{srs}</WKT></SpatialReference></DEFeatureClassInfo>"
        );
        let path = format!("\\{dataset}\\{name}");
        self.add_item(
            TYPE_FEATURE_CLASS,
            name,
            1,
            geometry_type.code(),
            &definition,
            &path,
            schema,
            rows,
        )
    }
}
