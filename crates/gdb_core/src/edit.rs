//! 编辑会话（`EditSession`）：参考 ArcEngine 的 `IWorkspaceEdit`。
//!
//! 通过会话打开的要素类/表会被登记，调用 `commit()` 时统一写回磁盘；
//! `abort()` 会**回滚**自最近一次 `start_operation()` 以来对这些表的内存改动
//! （依赖快照），并放弃写盘。
//!
//! ## 事务语义
//! - `start()`：开始编辑（`StartEditing`）。
//! - `start_operation()` / `stop_operation()`：编辑操作分组（`StartEditOperation` /
//!   `StopEditOperation`），支持嵌套；**最外层** `start_operation` 建立快照。
//! - `commit()`：要求已 `start` 且所有操作已 `stop`，然后 flush 全部登记表
//!   （`StopEditing(true)`）。
//! - `abort()`：恢复快照（`AbortEditOperation` 语义）并结束编辑（`StopEditing(false)`）。
//!
//! 注意：`abort()` 的回滚能力源自 `start_operation` 建立的快照；未开启操作时，
//! `abort()` 退化为「不写盘」（与 ArcEngine 一致）。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::error::{GdbError, Result};
use crate::feature_class::{FeatureClass, TableHandle};
use crate::table::{Table, TableSnapshot};
use crate::workspace::Geodatabase;

/// 编辑会话。
pub struct EditSession {
    tables: RefCell<Vec<Rc<RefCell<Table>>>>,
    started: Cell<bool>,
    /// 当前嵌套的编辑操作深度。
    op_depth: Cell<usize>,
    /// 各层操作名（可选，便于诊断）。
    op_stack: RefCell<Vec<String>>,
    /// 最外层 start_operation 时建立的行数据快照。
    snapshots: RefCell<Vec<(Rc<RefCell<Table>>, TableSnapshot)>>,
}

impl EditSession {
    pub(crate) fn new() -> Self {
        EditSession {
            tables: RefCell::new(Vec::new()),
            started: Cell::new(false),
            op_depth: Cell::new(0),
            op_stack: RefCell::new(Vec::new()),
            snapshots: RefCell::new(Vec::new()),
        }
    }

    /// 开始编辑（对应 `IWorkspaceEdit.StartEditing`）。
    pub fn start(&self) {
        self.started.set(true);
    }

    /// 是否处于编辑状态（对应 `IWorkspaceEdit.IsBeingEdited`）。
    pub fn is_being_edited(&self) -> bool {
        self.started.get()
    }

    /// 当前编辑操作嵌套深度。
    pub fn operation_depth(&self) -> usize {
        self.op_depth.get()
    }

    /// 开始一个编辑操作（对应 `StartEditOperation`），支持嵌套。
    ///
    /// 最外层调用会对当前已登记的表建立快照，以便 `abort()` 回滚。
    pub fn start_operation(&self) -> Result<()> {
        if !self.started.get() {
            return Err(GdbError::Edit(
                "尚未 start()，不能 start_operation()".into(),
            ));
        }
        let depth = self.op_depth.get();
        if depth == 0 {
            // 最外层：对已登记表建立快照。
            let mut snaps = self.snapshots.borrow_mut();
            snaps.clear();
            for t in self.tables.borrow().iter() {
                snaps.push((t.clone(), t.borrow().snapshot()));
            }
        }
        self.op_depth.set(depth + 1);
        self.op_stack.borrow_mut().push(String::new());
        Ok(())
    }

    /// 结束一个编辑操作（对应 `StopEditOperation`）。
    pub fn stop_operation(&self) -> Result<()> {
        let depth = self.op_depth.get();
        if depth == 0 {
            return Err(GdbError::Edit(
                "没有处于开启状态的编辑操作（stop_operation 不配对）".into(),
            ));
        }
        self.op_depth.set(depth - 1);
        self.op_stack.borrow_mut().pop();
        Ok(())
    }

    /// 便捷：`start()` + `start_operation()`，开启一个可回滚的事务。
    pub fn begin_transaction(&self) -> Result<()> {
        self.start();
        self.start_operation()
    }

    /// 登记一个已打开的表引用（避免重复登记）。
    ///
    /// 若当前处于编辑操作中，则同时为新登记的表补一次快照（保证其也能被回滚）。
    fn register(&self, table: Rc<RefCell<Table>>) {
        let already = self.tables.borrow().iter().any(|t| Rc::ptr_eq(t, &table));
        if already {
            return;
        }
        if self.op_depth.get() > 0 {
            let snap = table.borrow().snapshot();
            self.snapshots.borrow_mut().push((table.clone(), snap));
        }
        self.tables.borrow_mut().push(table);
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
    ///
    /// 要求已 `start()` 且所有 `start_operation` 都已配对 `stop_operation`。
    pub fn commit(&self) -> Result<()> {
        if !self.started.get() {
            return Err(GdbError::Edit("尚未 start()，不能 commit()".into()));
        }
        if self.op_depth.get() != 0 {
            return Err(GdbError::Edit(format!(
                "仍有 {} 层编辑操作未 stop_operation，不能 commit()",
                self.op_depth.get()
            )));
        }
        for t in self.tables.borrow().iter() {
            t.borrow().flush()?;
        }
        self.snapshots.borrow_mut().clear();
        self.started.set(false);
        Ok(())
    }

    /// 放弃编辑：回滚自最近一次 `start_operation()` 以来的内存改动，且不写盘
    /// （对应 `AbortEditOperation` + `StopEditing(false)`）。
    pub fn abort(&self) {
        // 恢复快照（若存在）。
        for (table, snap) in self.snapshots.borrow().iter() {
            table.borrow_mut().restore(snap);
        }
        self.snapshots.borrow_mut().clear();
        self.op_stack.borrow_mut().clear();
        self.op_depth.set(0);
        self.started.set(false);
    }
}
