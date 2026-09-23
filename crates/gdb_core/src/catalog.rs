//! 目录枚举：基于 GDB_SystemCatalog（`a00000001`）与 GDB_Items 两级目录，
//! 区分独立要素类、独立数据表、要素数据集（内的要素类）。
//!
//! 真实 .gdb 的目录结构（已实证）：
//! - `a00000001`（GDB_SystemCatalog）：仅 `ID/Name/FileFormat` 三字段，
//!   是**文件号 ↔ 内部表名**映射（`ID` → `a{ID:08x}.gdbtable`）。
//! - 用户对象注册表在 **GDB_Items**：每行含 `Name/PhysicalName/Path/Type(GUID)/
//!   DatasetSubtype1/DatasetSubtype2/Definition(XML)`。
//! - `Type` GUID → GDB_ItemTypes（`UUID/Name`）解析为 "Feature Class"/"Table"/
//!   "Feature Dataset" 等；Definition XML 前缀（`<DEFeatureClass*`/`<DETable`/
//!   `<DEFeatureDataset`）作为兜底判定。
//! - `DatasetSubtype2` 在要素类上为几何类型码（1=Point 2=MultiPoint 3=Polyline
//!   4=Polygon 9=MultiPatch…）。
//! - 数据文件号由 `PhysicalName`（或 `Name`）回查 SystemCatalog 得到；
//!   PhysicalName 大小写可能与目录记录不一致，做小写兜底匹配。

use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::{GdbError, Result};
use crate::field::GeometryType;
use crate::table::Table;
use crate::value::FieldValue;

/// 目录条目类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogItemType {
    FeatureClass,
    Table,
    FeatureDataset,
}

/// 一个目录条目（要素类 / 表 / 要素数据集）。
#[derive(Debug, Clone)]
pub struct CatalogItem {
    /// 条目名称（路径最后一段）。
    pub name: String,
    /// 完整路径（`\FD\FC` 形式）。
    pub path: String,
    /// 条目类型。
    pub item_type: CatalogItemType,
    /// 几何类型（仅要素类有意义）。
    pub geometry_type: GeometryType,
    /// 数据文件编号（`a{file_id:08x}.gdbtable`）。
    pub file_id: u32,
    /// 所属要素数据集名（要素数据集中的要素类才有）。
    pub parent_dataset: Option<String>,
}

fn field_text(v: &FieldValue) -> String {
    match v {
        FieldValue::Text(s) | FieldValue::Xml(s) => s.clone(),
        FieldValue::Null => String::new(),
        other => format!("{other:?}"),
    }
}

fn field_int(v: &FieldValue) -> i64 {
    match v {
        FieldValue::Int16(x) => *x as i64,
        FieldValue::Int32(x) => *x as i64,
        FieldValue::Int64(x) => *x,
        FieldValue::ObjectId(x) => *x as i64,
        _ => 0,
    }
}

/// 枚举 .gdb 目录中的所有用户对象（要素类 / 表 / 要素数据集）。
pub fn enumerate(directory: &PathBuf) -> Result<Vec<CatalogItem>> {
    // 1) SystemCatalog：内部表名 → 文件号。
    let catalog = Table::open(directory, 1)?;
    let cat_id_i = catalog
        .schema
        .field_index("ID")
        .ok_or_else(|| GdbError::Format("系统目录缺少 ID 字段".into()))?;
    let cat_name_i = catalog
        .schema
        .field_index("Name")
        .ok_or_else(|| GdbError::Format("系统目录缺少 Name 字段".into()))?;

    let mut name_to_id: HashMap<String, u32> = HashMap::new();
    for row in &catalog.rows {
        let name = field_text(&row[cat_name_i]);
        let id = field_int(&row[cat_id_i]) as u32;
        if !name.is_empty() {
            name_to_id.insert(name, id);
        }
    }
    // 大小写不敏感兜底映射（PhysicalName 与目录记录大小写可能不一致）。
    let lower_to_id: HashMap<String, u32> = name_to_id
        .iter()
        .map(|(k, v)| (k.to_lowercase(), *v))
        .collect();

    // 2) GDB_ItemTypes：UUID(16B) → 类型名。
    let mut type_names: HashMap<[u8; 16], String> = HashMap::new();
    if let Some(&tid) = name_to_id.get("GDB_ItemTypes") {
        if let Ok(t) = Table::open(directory, tid) {
            if let (Some(ui), Some(ni)) =
                (t.schema.field_index("UUID"), t.schema.field_index("Name"))
            {
                for row in &t.rows {
                    if let FieldValue::Uuid(u) = &row[ui] {
                        type_names.insert(u.0, field_text(&row[ni]));
                    }
                }
            }
        }
    }

    // 3) GDB_Items：用户对象注册表。
    let items_id = name_to_id
        .get("GDB_Items")
        .copied()
        .ok_or_else(|| GdbError::Format("系统目录缺少 GDB_Items 表".into()))?;
    let items = Table::open(directory, items_id)?;

    let name_i = items.schema.field_index("Name");
    let phys_i = items.schema.field_index("PhysicalName");
    let path_i = items.schema.field_index("Path");
    let type_i = items.schema.field_index("Type");
    let ds2_i = items.schema.field_index("DatasetSubtype2");
    let def_i = items.schema.field_index("Definition");

    let mut out = Vec::new();
    for row in &items.rows {
        let name = name_i.map(|i| field_text(&row[i])).unwrap_or_default();
        let physical = phys_i.map(|i| field_text(&row[i])).unwrap_or_default();
        let path = path_i.map(|i| field_text(&row[i])).unwrap_or_default();
        let def = def_i.map(|i| field_text(&row[i])).unwrap_or_default();
        let ds2 = ds2_i.map(|i| field_int(&row[i])) .unwrap_or(0) as i32;

        // Type GUID → 类型名。
        let type_name = type_i
            .and_then(|i| match &row[i] {
                FieldValue::Uuid(u) => type_names.get(&u.0).cloned(),
                _ => None,
            })
            .unwrap_or_default();

        // 跳过根对象、工作空间、文件夹等非数据对象。
        if path.trim_start_matches('\\').is_empty() {
            continue;
        }
        if matches!(type_name.as_str(), "Workspace" | "Folder") {
            continue;
        }

        // 由 Path 解析名称与父数据集："\FD\FC" -> parent=FD, name=FC。
        let raw = path.trim_start_matches('\\');
        let segs: Vec<&str> = raw.split('\\').filter(|s| !s.is_empty()).collect();
        let (parent, item_name) = match segs.len() {
            0 => (None, name.clone()),
            1 => (None, segs[0].to_string()),
            _ => (
                Some(segs[segs.len() - 2].to_string()),
                segs.last().unwrap().to_string(),
            ),
        };

        // 分类：Definition XML 前缀优先（最直接），Type GUID 兜底。
        let def_prefix = def.trim_start();
        let (item_type, geometry_type) = if def_prefix.starts_with("<DEFeatureDataset")
            || type_name == "Feature Dataset"
        {
            (CatalogItemType::FeatureDataset, GeometryType::None)
        } else if def_prefix.starts_with("<DEFeatureClass") || type_name == "Feature Class" {
            (CatalogItemType::FeatureClass, GeometryType::from_code(ds2))
        } else if def_prefix.starts_with("<DETable") || type_name == "Table" {
            (CatalogItemType::Table, GeometryType::None)
        } else {
            // 其它对象（Domain、Relationship Class、Info 等系统对象）不参与枚举。
            continue;
        };

        // PhysicalName/Name → SystemCatalog → 文件号。
        let file_id = name_to_id
            .get(&physical)
            .or_else(|| name_to_id.get(&name))
            .or_else(|| lower_to_id.get(&physical.to_lowercase()))
            .or_else(|| lower_to_id.get(&name.to_lowercase()))
            .copied();
        match (file_id, &item_type) {
            (Some(fid), _) => {
                out.push(CatalogItem {
                    name: item_name,
                    path,
                    item_type,
                    geometry_type,
                    file_id: fid,
                    parent_dataset: parent,
                });
            }
            // 要素数据集是虚拟容器，无数据文件是正常情况（file_id 置 0）。
            (None, CatalogItemType::FeatureDataset) => {
                out.push(CatalogItem {
                    name: item_name,
                    path,
                    item_type,
                    geometry_type,
                    file_id: 0,
                    parent_dataset: parent,
                });
            }
            // 其它找不到数据文件的注册项（异常）——跳过而非失败。
            (None, _) => {}
        }
    }
    Ok(out)
}
