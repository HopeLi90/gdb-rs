//! `gdb_core`：纯 Rust 实现的 ESRI File Geodatabase（.gdb）解析与管理库。
//!
//! 代码组织结构参考 ArcGIS ArcEngine 的数据库要素类遍历/更新模型：
//! `Geodatabase`（工作空间）→ `FeatureClass` / `TableHandle` / `FeatureDataset`
//! → `Cursor`（search/update/insert）→ `Row` / `Feature` → `store()`，
//! 以及 `EditSession`（start/start_operation/stop_operation/commit/abort）。

pub mod builder;
pub mod catalog;
pub mod cursor;
pub mod edit;
pub mod error;
pub mod feature;
pub mod feature_class;
pub mod feature_dataset;
pub mod field;
pub mod geometry;
pub mod io;
pub mod row;
pub mod table;
pub mod value;
pub mod workspace;
pub mod xml;

pub use catalog::{CatalogItem, CatalogItemType};
pub use cursor::{Cursor, CursorMode, QueryFilter};
pub use edit::EditSession;
pub use error::{GdbError, Result};
pub use feature::Feature;
pub use feature_class::{FeatureClass, TableHandle};
pub use feature_dataset::FeatureDataset;
pub use field::{FieldDef, FieldType, GeometryType, PrecisionGrid, TableSchema};
pub use geometry::Geometry;
pub use row::Row;
pub use table::Table;
pub use value::FieldValue;
pub use workspace::Geodatabase;

/// 便捷构造：以默认精度网格新建一个带 OBJECTID 字段的模式骨架。
pub fn base_schema(geometry_type: field::GeometryType, has_z: bool, has_m: bool) -> TableSchema {
    let mut fields = Vec::new();
    let mut oid = FieldDef::new("OBJECTID", FieldType::ObjectId);
    oid.nullable = false;
    fields.push(oid);
    TableSchema {
        geometry_type,
        has_z,
        has_m,
        string_utf8: true,
        fields,
    }
}
