//! 工作空间（`Geodatabase`）：参考 ArcEngine 的 `IWorkspaceFactory.Open` /
//! `IFeatureWorkspace`，负责打开 .gdb 并列举、打开其中的要素类/表/要素数据集。

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::catalog::{CatalogItem, CatalogItemType, enumerate};
use crate::error::{GdbError, Result};
use crate::feature_class::{FeatureClass, TableHandle};
use crate::feature_dataset::FeatureDataset;
use crate::table::Table;

/// File Geodatabase 工作空间。
pub struct Geodatabase {
    root: PathBuf,
    items: Vec<CatalogItem>,
}

impl Geodatabase {
    /// 打开一个 .gdb 目录（必须是含 `a00000001.gdbtable` 的文件夹）。
    pub fn open(root: &Path) -> Result<Geodatabase> {
        let p_table = root.join("a00000001.gdbtable");
        let p_tablex = root.join("a00000001.gdbtablx");
        if !(p_table.exists() && p_tablex.exists()) {
            return Err(GdbError::NotAGeodatabase(format!(
                "{} 不是合法的 File Geodatabase（缺少 a00000001 系统目录）",
                root.display()
            )));
        }
        let items = enumerate(&root.to_path_buf())?;
        Ok(Geodatabase {
            root: root.to_path_buf(),
            items,
        })
    }

    /// 目录根路径。
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// 全部目录条目。
    pub fn items(&self) -> &[CatalogItem] {
        &self.items
    }

    /// 独立要素类 + 要素数据集中的要素类。
    pub fn feature_classes(&self) -> Vec<&CatalogItem> {
        self.items
            .iter()
            .filter(|i| i.item_type == CatalogItemType::FeatureClass)
            .collect()
    }

    /// 独立数据表。
    pub fn tables(&self) -> Vec<&CatalogItem> {
        self.items
            .iter()
            .filter(|i| i.item_type == CatalogItemType::Table)
            .collect()
    }

    /// 要素数据集。
    pub fn feature_datasets(&self) -> Vec<&CatalogItem> {
        self.items
            .iter()
            .filter(|i| i.item_type == CatalogItemType::FeatureDataset)
            .collect()
    }

    fn find(&self, ty: CatalogItemType, name: &str) -> Result<CatalogItem> {
        self.items
            .iter()
            .find(|i| i.item_type == ty && i.name == name)
            .cloned()
            .ok_or_else(|| GdbError::NotFound(name.to_string()))
    }

    /// 打开要素类（独立或要素数据集中的均可，按名称匹配）。
    pub fn open_feature_class(&self, name: &str) -> Result<FeatureClass> {
        let item = self.find(CatalogItemType::FeatureClass, name)?;
        let table = Table::open(&self.root, item.file_id)?;
        Ok(FeatureClass::new(Rc::new(RefCell::new(table)), item))
    }

    /// 打开独立数据表。
    pub fn open_table(&self, name: &str) -> Result<TableHandle> {
        let item = self.find(CatalogItemType::Table, name)?;
        let table = Table::open(&self.root, item.file_id)?;
        Ok(TableHandle::new(Rc::new(RefCell::new(table)), item))
    }

    /// 打开要素数据集（返回容器，可进一步打开其下要素类）。
    pub fn open_feature_dataset(&self, name: &str) -> Result<FeatureDataset> {
        let item = self.find(CatalogItemType::FeatureDataset, name)?;
        Ok(FeatureDataset { item })
    }

    /// 新建一个编辑会话（参考 `IWorkspaceEdit`）。
    pub fn edit_session(&self) -> crate::edit::EditSession {
        crate::edit::EditSession::new()
    }
}
