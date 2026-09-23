//! 游标（`Cursor`）：参考 ArcEngine 的 `Search`/`Update`/`Insert` 遍历模式。

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{GdbError, Result};
use crate::feature::Feature;
use crate::row::Row;
use crate::table::Table;
use crate::value::FieldValue;

/// 查询过滤条件（最小实现：全部 / 按 OBJECTID）。
#[derive(Debug, Clone)]
pub enum QueryFilter {
    /// 遍历全部行。
    All,
    /// 仅匹配指定 OBJECTID（用于"更新指定要素"）。
    ByOid(u64),
}

impl QueryFilter {
    fn matches(&self, oid: u64) -> bool {
        match self {
            QueryFilter::All => true,
            QueryFilter::ByOid(target) => *target == oid,
        }
    }
}

/// 游标模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMode {
    Search,
    Update,
    Insert,
}

/// 表/要素类的遍历游标（持有共享表引用）。
pub struct Cursor {
    table: Rc<RefCell<Table>>,
    mode: CursorMode,
    indices: Vec<usize>,
    pos: usize,
}

impl Cursor {
    pub(crate) fn new(
        table: Rc<RefCell<Table>>,
        mode: CursorMode,
        filter: &QueryFilter,
    ) -> Result<Self> {
        let t = table.borrow();
        let oi = t.schema.objectid_index();
        let mut indices = Vec::new();
        for (i, row) in t.rows.iter().enumerate() {
            let oid = match oi {
                Some(oi) => match row[oi] {
                    FieldValue::ObjectId(v) => v,
                    _ => 0,
                },
                None => i as u64,
            };
            if filter.matches(oid) {
                indices.push(i);
            }
        }
        drop(t);
        Ok(Cursor {
            table,
            mode,
            indices,
            pos: 0,
        })
    }

    /// 是否还有下一项。
    pub fn has_next(&self) -> bool {
        self.pos < self.indices.len()
    }

    /// 取下一项行（只读/更新游标）。
    pub fn next_row(&mut self) -> Option<Row> {
        if self.pos >= self.indices.len() {
            return None;
        }
        let idx = self.indices[self.pos];
        self.pos += 1;
        Some(Row::new(self.table.clone(), idx))
    }

    /// 取下一项要素（仅对含几何的表有意义）。
    pub fn next_feature(&mut self) -> Option<Feature> {
        self.next_row().map(Feature::new)
    }

    /// 将行改动写回表（内存中 `set` 已即时生效，此处再次同步值以保证语义一致）。
    /// 仅允许在 Update 游标上调用（与 ArcEngine `IFeatureCursor` 语义一致）。
    pub fn update(&self, row: &Row) -> Result<()> {
        if self.mode != CursorMode::Update {
            return Err(GdbError::Edit("update 仅允许在 Update 游标上调用".into()));
        }
        // 先取出值副本（借用共享表），再取可变借用写回，避免 RefCell 重入借用。
        let values = row.values();
        let mut t = self.table.borrow_mut();
        if row.index < t.rows.len() {
            t.rows[row.index] = values;
        }
        Ok(())
    }

    /// 插入一行（Insert 游标）。自动分配新 OBJECTID。
    pub fn insert(&mut self, row: &Row) -> Result<()> {
        if self.mode != CursorMode::Insert {
            return Err(GdbError::Edit("insert 仅允许在 Insert 游标上调用".into()));
        }
        // 先取出值副本，避免后续可变借用期间重入借用同一表。
        let mut values = row.values();
        let mut t = self.table.borrow_mut();
        let oi = t.schema.objectid_index();
        if let Some(oi) = oi {
            let new_oid = t
                .rows
                .iter()
                .filter_map(|r| match r[oi] {
                    FieldValue::ObjectId(v) => Some(v),
                    _ => None,
                })
                .max()
                .map(|m| m + 1)
                .unwrap_or(1);
            values[oi] = FieldValue::ObjectId(new_oid);
        }
        let idx = t.rows.len();
        t.rows.push(values);
        self.indices.push(idx);
        Ok(())
    }
}
