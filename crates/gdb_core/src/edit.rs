//! 编辑会话（`EditSession`）：参考 ArcEngine 的 `IWorkspaceEdit`。
//!
//! 通过会话打开的要素类/表会被登记，调用 `commit()` 时统一写回磁盘；
//! `abort()` 不写盘（内存句柄中的未保存改动仍保留，但不会被持久化）。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::error::Result;
use crate::feature_class::{FeatureClass, TableHandle};
use crate::table::Table;
use crate::workspace::Geodatabase;

/// 编辑会话。
pub struct EditSession {
    tables: RefCell<Vec<Rc<RefCell<Table>>>>,
    started: Cell<bool>,
}

impl EditSession {
    pub(crate) fn new() -> Self {
        EditSession {
            tables: RefCell::new(Vec::new()),
            started: Cell::new(false),
        }
    }

    /// 开始编辑（对应 `IWorkspaceEdit.StartEditing`）。
    pub fn start(&self) {
        self.started.set(true);
    }

    /// 开始一个编辑操作（对应 `StartEditOperation`），用于分组多步改动。
    pub fn start_operation(&self) {}

    /// 结束一个编辑操作（对应 `StopEditOperation`）。
    pub fn stop_operation(&self) {}

    /// 登记一个已打开的表引用（避免重复登记）。
    fn register(&self, table: Rc<RefCell<Table>>) {
        if !self.tables.borrow().iter().any(|t| Rc::ptr_eq(t, &table)) {
            self.tables.borrow_mut().push(table);
        }
    }

    /// 在会话内打开要素类并登记。
    pub fn open_feature_class(&self, gdb: &Geodatabase, name: &str) -> Result<FeatureClass> {
        let fc = gdb.open_feature_class(name)?;
        self.register(fc.table_rc());
        Ok(fc)
    }

    /// 在会话内打开表并登记。
    pub fn open_table(&self, gdb: &Geodatabase, name: &str) -> Result<TableHandle> {
        let t = gdb.open_table(name)?;
        self.register(t.table_rc());
        Ok(t)
    }

    /// 提交编辑：将所有登记的表写回磁盘（对应 `StopEditing(true)`）。
    pub fn commit(&self) -> Result<()> {
        self.started.set(false);
        for t in self.tables.borrow().iter() {
            t.borrow().flush()?;
        }
        Ok(())
    }

    /// 放弃编辑：不写盘（对应 `StopEditing(false)`）。
    pub fn abort(&self) {
        self.started.set(false);
    }
}
