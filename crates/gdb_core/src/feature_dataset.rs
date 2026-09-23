//! 要素数据集（`FeatureDataset`）：要素类的容器，可列出并打开其下的要素类。

use crate::catalog::CatalogItem;
use crate::error::{GdbError, Result};
use crate::feature_class::FeatureClass;
use crate::workspace::Geodatabase;

/// 要素数据集（虚拟容器，聚合其下的要素类）。
#[derive(Clone)]
pub struct FeatureDataset {
    pub item: CatalogItem,
}

impl FeatureDataset {
    /// 数据集名称。
    pub fn name(&self) -> &str {
        &self.item.name
    }

    /// 打开该数据集下的全部要素类。
    pub fn feature_classes(&self, gdb: &Geodatabase) -> Result<Vec<FeatureClass>> {
        let mut out = Vec::new();
        for item in gdb.items() {
            if item.item_type == crate::catalog::CatalogItemType::FeatureClass
                && item.parent_dataset.as_deref() == Some(self.item.name.as_str())
            {
                out.push(gdb.open_feature_class(&item.name)?);
            }
        }
        Ok(out)
    }

    /// 按名称打开该数据集下的某个要素类。
    pub fn open_feature_class(&self, gdb: &Geodatabase, name: &str) -> Result<FeatureClass> {
        let target = gdb
            .items()
            .iter()
            .find(|item| {
                item.item_type == crate::catalog::CatalogItemType::FeatureClass
                    && item.parent_dataset.as_deref() == Some(self.item.name.as_str())
                    && item.name == name
            })
            .ok_or_else(|| GdbError::NotFound(format!("要素数据集 {} 中未找到要素类 {name}", self.item.name)))?;
        gdb.open_feature_class(&target.name)
    }
}
