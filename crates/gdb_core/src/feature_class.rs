//! 要素类（`FeatureClass`）与属性表（`TableHandle`）：ArcEngine 风格的访问入口，
//! 提供 `search`/`update`/`insert` 游标工厂与 schema 访问。

use std::cell::RefCell;
use std::rc::Rc;

use crate::catalog::CatalogItem;
use crate::cursor::{Cursor, CursorMode, QueryFilter};
use crate::error::Result;
use crate::field::{FieldDef, GeometryType};
use crate::table::Table;

/// 要素类（含几何）。持有共享的表引用与目录条目。
#[derive(Clone)]
pub struct FeatureClass {
    pub(crate) table: Rc<RefCell<Table>>,
    pub item: CatalogItem,
}

impl FeatureClass {
    pub(crate) fn new(table: Rc<RefCell<Table>>, item: CatalogItem) -> Self {
        FeatureClass { table, item }
    }

    /// 要素类名称。
    pub fn name(&self) -> &str {
        &self.item.name
    }

    /// 要素数量。
    pub fn feature_count(&self) -> usize {
        self.table.borrow().rows.len()
    }

    /// 字段定义列表。
    pub fn fields(&self) -> Vec<FieldDef> {
        self.table.borrow().schema.fields.clone()
    }

    /// 几何类型。
    pub fn shape_type(&self) -> GeometryType {
        self.table.borrow().schema.geometry_type
    }

    /// 空间参考 WKT（来自几何字段）。
    pub fn spatial_reference(&self) -> String {
        let t = self.table.borrow();
        t.schema
            .geometry_index()
            .and_then(|i| t.schema.fields.get(i))
            .map(|f| f.srs_wkt.clone())
            .unwrap_or_default()
    }

    /// 搜索游标（遍历要素）。
    pub fn search(&self, filter: &QueryFilter) -> Result<Cursor> {
        Cursor::new(self.table.clone(), CursorMode::Search, filter)
    }

    /// 更新游标（用于按 OBJECTID 等修改要素）。
    pub fn update(&self, filter: &QueryFilter) -> Result<Cursor> {
        Cursor::new(self.table.clone(), CursorMode::Update, filter)
    }

    /// 插入游标。
    pub fn insert(&self) -> Result<Cursor> {
        Cursor::new(self.table.clone(), CursorMode::Insert, &QueryFilter::All)
    }

    /// 将内存中的改动写回磁盘。
    pub fn save(&self) -> Result<()> {
        self.table.borrow().flush()
    }

    /// 暴露底层表引用（编辑会话注册用）。
    pub(crate) fn table_rc(&self) -> Rc<RefCell<Table>> {
        self.table.clone()
    }
}

/// 独立数据表（无几何）。
#[derive(Clone)]
pub struct TableHandle {
    pub(crate) table: Rc<RefCell<Table>>,
    pub item: CatalogItem,
}

impl TableHandle {
    pub(crate) fn new(table: Rc<RefCell<Table>>, item: CatalogItem) -> Self {
        TableHandle { table, item }
    }

    /// 表名称。
    pub fn name(&self) -> &str {
        &self.item.name
    }

    /// 行数。
    pub fn row_count(&self) -> usize {
        self.table.borrow().rows.len()
    }

    /// 字段定义列表。
    pub fn fields(&self) -> Vec<FieldDef> {
        self.table.borrow().schema.fields.clone()
    }

    /// 搜索游标（遍历行）。
    pub fn search(&self, filter: &QueryFilter) -> Result<Cursor> {
        Cursor::new(self.table.clone(), CursorMode::Search, filter)
    }

    /// 更新游标。
    pub fn update(&self, filter: &QueryFilter) -> Result<Cursor> {
        Cursor::new(self.table.clone(), CursorMode::Update, filter)
    }

    /// 插入游标。
    pub fn insert(&self) -> Result<Cursor> {
        Cursor::new(self.table.clone(), CursorMode::Insert, &QueryFilter::All)
    }

    /// 写回磁盘。
    pub fn save(&self) -> Result<()> {
        self.table.borrow().flush()
    }

    pub(crate) fn table_rc(&self) -> Rc<RefCell<Table>> {
        self.table.clone()
    }
}
