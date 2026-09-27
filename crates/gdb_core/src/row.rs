//! 行（`Row`）：按索引/名称取值与赋值。通过共享 `Rc<RefCell<Table>>` 实现
//! ArcEngine 风格的就地修改、`store()` 与 `delete()`。

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{GdbError, Result};
use crate::table::Table;
use crate::value::FieldValue;

/// 一行记录（引用共享的表，按索引就地修改）。
#[derive(Clone)]
pub struct Row {
    pub(crate) table: Rc<RefCell<Table>>,
    pub(crate) index: usize,
}

impl Row {
    pub(crate) fn new(table: Rc<RefCell<Table>>, index: usize) -> Self {
        Row { table, index }
    }

    /// 行在表中的位置。
    pub fn index(&self) -> usize {
        self.index
    }

    /// 该行是否仍存在于表中（删除后其下标可能失效）。
    pub fn is_valid(&self) -> bool {
        let t = self.table.borrow();
        self.index < t.rows.len()
    }

    /// 该行的 OBJECTID（行已失效时返回 0）。
    pub fn object_id(&self) -> u64 {
        let t = self.table.borrow();
        let oi = match t.schema.objectid_index() {
            Some(i) => i,
            None => return 0,
        };
        match t.rows.get(self.index).and_then(|r| r.get(oi)) {
            Some(FieldValue::ObjectId(v)) => *v,
            _ => 0,
        }
    }

    /// 按字段索引取值（克隆）；行已失效时返回 `FieldValue::Null`。
    pub fn get(&self, i: usize) -> FieldValue {
        self.table
            .borrow()
            .rows
            .get(self.index)
            .and_then(|r| r.get(i))
            .cloned()
            .unwrap_or(FieldValue::Null)
    }

    /// 按字段名取值。
    pub fn get_by_name(&self, name: &str) -> Result<FieldValue> {
        let t = self.table.borrow();
        let i = t
            .schema
            .field_index(name)
            .ok_or_else(|| GdbError::InvalidField(name.to_string()))?;
        Ok(t.rows
            .get(self.index)
            .and_then(|r| r.get(i))
            .cloned()
            .unwrap_or(FieldValue::Null))
    }

    /// 按字段索引赋值（立即写入内存中的表）。
    pub fn set(&self, i: usize, v: FieldValue) {
        let mut t = self.table.borrow_mut();
        if let Some(row) = t.rows.get_mut(self.index) {
            if i < row.len() {
                row[i] = v;
            }
        }
    }

    /// 按字段名赋值。
    pub fn set_by_name(&self, name: &str, v: FieldValue) -> Result<()> {
        let mut t = self.table.borrow_mut();
        let i = t
            .schema
            .field_index(name)
            .ok_or_else(|| GdbError::InvalidField(name.to_string()))?;
        if let Some(row) = t.rows.get_mut(self.index) {
            row[i] = v;
        }
        Ok(())
    }

    /// 写回（内存中修改已即时生效，此处为兼容 ArcEngine `IRow.Store` 语义的空操作）。
    pub fn store(&self) {}

    /// 删除本行（对应 ArcEngine `IRow.Delete`），返回被删行的 OBJECTID。
    ///
    /// 采用**保槽删除**：其余行的 OBJECTID 保持不变。删除后本 `Row` 句柄失效
    /// （`is_valid()` 返回 false）；若在游标中迭代删除，请改用
    /// [`crate::cursor::Cursor::delete_row`] 以同步维护游标状态。
    pub fn delete(&self) -> Result<u64> {
        self.table.borrow_mut().delete_row(self.index)
    }

    /// 取出该行的全部值副本；行已失效时返回空 Vec。
    pub fn values(&self) -> Vec<FieldValue> {
        self.table
            .borrow()
            .rows
            .get(self.index)
            .cloned()
            .unwrap_or_default()
    }
}
