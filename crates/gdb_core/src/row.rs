//! 行（`Row`）：按索引/名称取值与赋值。通过共享 `Rc<RefCell<Table>>` 实现
//! ArcEngine 风格的就地修改与 `store()`。

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

    /// 该行的 OBJECTID。
    pub fn object_id(&self) -> u64 {
        let t = self.table.borrow();
        let oi = match t.schema.objectid_index() {
            Some(i) => i,
            None => return 0,
        };
        match &t.rows[self.index][oi] {
            FieldValue::ObjectId(v) => *v,
            _ => 0,
        }
    }

    /// 按字段索引取值（克隆）。
    pub fn get(&self, i: usize) -> FieldValue {
        self.table.borrow().rows[self.index][i].clone()
    }

    /// 按字段名取值。
    pub fn get_by_name(&self, name: &str) -> Result<FieldValue> {
        let t = self.table.borrow();
        let i = t
            .schema
            .field_index(name)
            .ok_or_else(|| GdbError::InvalidField(name.to_string()))?;
        Ok(t.rows[self.index][i].clone())
    }

    /// 按字段索引赋值（立即写入内存中的表）。
    pub fn set(&self, i: usize, v: FieldValue) {
        self.table.borrow_mut().rows[self.index][i] = v;
    }

    /// 按字段名赋值。
    pub fn set_by_name(&self, name: &str, v: FieldValue) -> Result<()> {
        let mut t = self.table.borrow_mut();
        let i = t
            .schema
            .field_index(name)
            .ok_or_else(|| GdbError::InvalidField(name.to_string()))?;
        t.rows[self.index][i] = v;
        Ok(())
    }

    /// 写回（内存中修改已即时生效，此处为兼容 ArcEngine `IRow.Store` 语义的空操作）。
    pub fn store(&self) {}

    /// 取出该行的全部值副本。
    pub fn values(&self) -> Vec<FieldValue> {
        self.table.borrow().rows[self.index].clone()
    }
}
