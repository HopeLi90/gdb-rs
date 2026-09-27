//! 要素类（`FeatureClass`）与属性表（`TableHandle`）：ArcEngine 风格的访问入口，
//! 提供 `search`/`update`/`insert` 游标工厂、`create_*`/`delete_*`/`get_*` 便捷方法
//! 与 schema 访问。
//!
//! 命名对齐 ArcEngine：`IFeatureClass.CreateFeature`/`DeleteFeature`/`GetFeature`、
//! `ITable.CreateRow`/`DeleteSearchedRows`、`IRow.Delete` / `IFeatureCursor.DeleteFeature`。
//!
//! 所有写操作**只改内存**，需经 `save()` / [`crate::edit::EditSession::commit`] 才持久化。

use std::cell::RefCell;
use std::rc::Rc;

use crate::catalog::CatalogItem;
use crate::cursor::{Cursor, CursorMode, QueryFilter};
use crate::error::{GdbError, Result};
use crate::feature::Feature;
use crate::field::{FieldDef, GeometryType};
use crate::row::Row;
use crate::table::Table;
use crate::value::FieldValue;

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

    /// 更新游标（用于按条件修改/删除要素）。
    pub fn update(&self, filter: &QueryFilter) -> Result<Cursor> {
        Cursor::new(self.table.clone(), CursorMode::Update, filter)
    }

    /// 插入游标。
    pub fn insert(&self) -> Result<Cursor> {
        Cursor::new(self.table.clone(), CursorMode::Insert, &QueryFilter::All)
    }

    /// 按 OBJECTID 取要素（对应 `IFeatureClass.GetFeature`）。
    pub fn get_feature(&self, oid: u64) -> Result<Option<Feature>> {
        let mut cur = self.search(&QueryFilter::ByOid(oid))?;
        Ok(cur.next_feature())
    }

    /// 查询满足条件的全部 OBJECTID（按升序）。
    pub fn query_oids(&self, filter: &QueryFilter) -> Result<Vec<u64>> {
        let t = self.table.borrow();
        let indices = Cursor::matching_indices(&self.table, filter)?;
        Ok(indices
            .into_iter()
            .filter_map(|i| t.oid_at(i))
            .collect())
    }

    /// 新建要素（对应 `IFeatureClass.CreateFeature`），返回新分配的 OBJECTID。
    ///
    /// `values` 的字段顺序须与 schema 一致；OBJECTID 位会被自动覆盖。
    pub fn create_feature(&self, values: Vec<FieldValue>) -> Result<u64> {
        create_row_in(&self.table, values)
    }

    /// 按 OBJECTID 删除要素（对应 `IFeatureClass.DeleteFeature` / `IRow.Delete`）。
    /// 未找到返回 `Ok(false)`。
    pub fn delete_feature(&self, oid: u64) -> Result<bool> {
        self.table.borrow_mut().delete_row_by_oid(oid)
    }

    /// 删除满足过滤条件的要素（对应 `ITable.DeleteSearchedRows`），返回删除数。
    pub fn delete_searched_rows(&self, filter: &QueryFilter) -> Result<usize> {
        let indices = Cursor::matching_indices(&self.table, filter)?;
        self.table.borrow_mut().delete_rows(&indices)
    }

    /// 按 OBJECTID 列表批量删除，返回删除数。
    pub fn delete_rows(&self, oids: &[u64]) -> Result<usize> {
        let mut t = self.table.borrow_mut();
        let mut count = 0usize;
        for &oid in oids {
            if t.delete_row_by_oid(oid)? {
                count += 1;
            }
        }
        Ok(count)
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

    /// 按 OBJECTID 取行。
    pub fn get_row(&self, oid: u64) -> Result<Option<Row>> {
        let mut cur = self.search(&QueryFilter::ByOid(oid))?;
        Ok(cur.next_row())
    }

    /// 查询满足条件的全部 OBJECTID（按升序）。
    pub fn query_oids(&self, filter: &QueryFilter) -> Result<Vec<u64>> {
        let t = self.table.borrow();
        let indices = Cursor::matching_indices(&self.table, filter)?;
        Ok(indices
            .into_iter()
            .filter_map(|i| t.oid_at(i))
            .collect())
    }

    /// 新建行（对应 `ITable.CreateRow`），返回新分配的 OBJECTID。
    pub fn create_row(&self, values: Vec<FieldValue>) -> Result<u64> {
        create_row_in(&self.table, values)
    }

    /// 按 OBJECTID 删除行（对应 `ITable.DeleteRows` / `IRow.Delete`）。
    /// 未找到返回 `Ok(false)`。
    pub fn delete_row_by_oid(&self, oid: u64) -> Result<bool> {
        self.table.borrow_mut().delete_row_by_oid(oid)
    }

    /// 删除满足过滤条件的行（对应 `ITable.DeleteSearchedRows`），返回删除数。
    pub fn delete_searched_rows(&self, filter: &QueryFilter) -> Result<usize> {
        let indices = Cursor::matching_indices(&self.table, filter)?;
        self.table.borrow_mut().delete_rows(&indices)
    }

    /// 按 OBJECTID 列表批量删除，返回删除数。
    pub fn delete_rows(&self, oids: &[u64]) -> Result<usize> {
        let mut t = self.table.borrow_mut();
        let mut count = 0usize;
        for &oid in oids {
            if t.delete_row_by_oid(oid)? {
                count += 1;
            }
        }
        Ok(count)
    }

    /// 写回磁盘。
    pub fn save(&self) -> Result<()> {
        self.table.borrow().flush()
    }

    pub(crate) fn table_rc(&self) -> Rc<RefCell<Table>> {
        self.table.clone()
    }
}

/// 在表中追加一行并返回新 OBJECTID（`FeatureClass::create_feature` /
/// `TableHandle::create_row` 的共用实现）。
///
/// OBJECTID 由槽位合成（`OID = 槽位 + 1`），新行追加到 `slot_count` 槽位。
pub(crate) fn create_row_in(table: &Rc<RefCell<Table>>, values: Vec<FieldValue>) -> Result<u64> {
    let mut t = table.borrow_mut();
    if values.len() != t.schema.fields.len() {
        return Err(GdbError::Format(format!(
            "行字段数 {} 与 schema {} 不一致",
            values.len(),
            t.schema.fields.len()
        )));
    }
    let mut values = values;
    let new_oid = t.slot_count + 1;
    if let Some(oi) = t.schema.objectid_index() {
        values[oi] = FieldValue::ObjectId(new_oid);
    }
    t.add_row(values)?;
    Ok(new_oid)
}
