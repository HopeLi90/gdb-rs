//! 游标（`Cursor`）：参考 ArcEngine 的 `Search`/`Update`/`Insert` 遍历模式。
//!
//! `QueryFilter` 已迁至 [`crate::query_filter`]，此处 re-export 以保持既有路径
//! （`gdb_core::cursor::QueryFilter`）兼容。

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{GdbError, Result};
use crate::feature::Feature;
use crate::row::Row;
use crate::table::Table;
use crate::value::FieldValue;

pub use crate::query_filter::{QueryFilter, SpatialFilter, SpatialRel, WhereClause};

/// 游标模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMode {
    /// 只读遍历。
    Search,
    /// 可更新、可删除。
    Update,
    /// 可插入。
    Insert,
}

/// 表/要素类的遍历游标（持有共享表引用）。
///
/// 迭代中删除的正确范式（与 ArcEngine「先 Search 收集、再 Update 删」一致）：
/// ```ignore
/// let mut victims = Vec::new();
/// {
///     let mut sc = fc.search(&QueryFilter::All)?;
///     while let Some(f) = sc.next_feature() { victims.push(f); }
/// }
/// let mut uc = fc.update(&QueryFilter::All)?;
/// for f in &victims { uc.delete_feature(f)?; }
/// ```
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
        let indices = Self::matching_indices(&table, filter)?;
        Ok(Cursor {
            table,
            mode,
            indices,
            pos: 0,
        })
    }

    /// 计算满足过滤条件的行索引集合。
    ///
    /// 对「仅按 OID」的简单条件走零克隆快路径；复杂条件（属性/空间）先取表快照副本，
    /// 避免求值期间重入 `RefCell` 借用。
    pub(crate) fn matching_indices(
        table: &Rc<RefCell<Table>>,
        filter: &QueryFilter,
    ) -> Result<Vec<usize>> {
        if filter.is_oid_only() {
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
                if filter.matches_row(&[], &t.schema, oid, None) {
                    indices.push(i);
                }
            }
            return Ok(indices);
        }

        // 复杂条件：取快照避免重入借用。
        let (rows, schema, oi, gi) = {
            let t = table.borrow();
            (
                t.rows.clone(),
                t.schema.clone(),
                t.schema.objectid_index(),
                t.schema.geometry_index(),
            )
        };
        let mut indices = Vec::with_capacity(rows.len());
        for (i, row) in rows.iter().enumerate() {
            let oid = match oi {
                Some(oi) => match row[oi] {
                    FieldValue::ObjectId(v) => v,
                    _ => (i as u64) + 1,
                },
                None => (i as u64) + 1,
            };
            let geom = gi.and_then(|gi| match &row[gi] {
                FieldValue::Geometry(g) => Some(g),
                _ => None,
            });
            if filter.matches_row(row, &schema, oid, geom) {
                indices.push(i);
            }
        }
        Ok(indices)
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
    ///
    /// OBJECTID 由 `.gdbtablx` 槽位合成（`OID = 槽位 + 1`），因此新行追加到
    /// `slot_count` 槽位，其 OBJECTID = `slot_count + 1`；同时必须同步维护
    /// `row_slots`（经 [`Table::add_row`]）。
    pub fn insert(&mut self, row: &Row) -> Result<()> {
        if self.mode != CursorMode::Insert {
            return Err(GdbError::Edit("insert 仅允许在 Insert 游标上调用".into()));
        }
        let values = row.values();
        let mut t = self.table.borrow_mut();
        let oi = t.schema.objectid_index();
        let mut values = values;
        if let Some(oi) = oi {
            // 新行将落在槽位 slot_count，故 OID = slot_count + 1。
            values[oi] = FieldValue::ObjectId(t.slot_count + 1);
        }
        let idx = t.rows.len();
        t.add_row(values)?;
        self.indices.push(idx);
        Ok(())
    }

    /// 删除一行（仅 Update 游标；对应 ArcEngine `IRow.Delete`）。
    pub fn delete_row(&mut self, row: &Row) -> Result<()> {
        if self.mode != CursorMode::Update {
            return Err(GdbError::Edit("delete 仅允许在 Update 游标上调用".into()));
        }
        let removed = row.index;
        self.table.borrow_mut().delete_row(removed)?;
        let (indices, pos) = remap(self.indices.clone(), self.pos, removed);
        self.indices = indices;
        self.pos = pos;
        Ok(())
    }
    /// 删除一个要素（仅 Update 游标；对应 ArcEngine `IFeatureCursor.DeleteFeature`）。
    pub fn delete_feature(&mut self, feature: &Feature) -> Result<()> {
        let row = feature.row().clone();
        self.delete_row(&row)
    }
}

/// 游标索引在删除表第 `removed` 行后的重定位。
///
/// 契约：`pos` 为**已消费（已由 `next_row` 返回）的匹配项数**，即下一次调用
/// 将返回 `indices[pos]`（若存在）。表内删除第 `removed` 行后，所有 `> removed`
/// 的行下标前移 1。
///
/// 处理三类情形：
/// 1. **删除已消费项**（该项在 `indices[..pos]` 中）：`pos` 减 1，使下一次仍指向
///    紧随其后的那一项，不跳过；
/// 2. **删除未消费项**（在 `indices[pos..]` 中）：`pos` 保持，仅移除该项并左移后续；
/// 3. **删除不匹配项**（`removed` 不在 `indices` 中）：集合仅左移，`pos` 按「新的
///    未消费起点」修正。
///
/// 返回 `(新 indices, 新 pos)`。
pub(crate) fn remap(indices: Vec<usize>, pos: usize, removed: usize) -> (Vec<usize>, usize) {
    let consumed = pos.min(indices.len());

    // 被删项在匹配集合中的位置（若存在）。
    let match_pos = indices.iter().position(|&i| i == removed);

    // 重建索引集合：丢弃 removed，并将 > removed 的项左移 1。
    let mut out: Vec<usize> = Vec::with_capacity(indices.len().saturating_sub(1));
    for &i in &indices {
        if i == removed {
            continue;
        }
        out.push(if i > removed { i - 1 } else { i });
    }

    // 定位新的 pos：已消费项（removed 之外）在 out 中的个数即为新 pos。
    // - 删除已消费项：其不在 out 中，故计数自然减 1（等价于 pos-1）。
    // - 删除未消费项：所有已消费项仍在 out 中，计数不变（等价于 pos）。
    // - 删除不匹配项：已消费项中 < removed 的仍在 out，> removed 的左移但仍存在，
    //   计数不变；若 removed 落在两者之间亦无须调整。
    let consumed_kept = if let Some(mp) = match_pos {
        if mp < consumed {
            consumed - 1
        } else {
            consumed
        }
    } else {
        consumed
    };

    let new_pos = consumed_kept.min(out.len());
    (out, new_pos)
}

#[cfg(test)]
mod tests {
    use super::remap;

    #[test]
    fn remap_delete_current() {
        // indices=[0,1,2,3], pos=1（已消费 [0]）。删表行 1（匹配集合位置 1，未消费）。
        let (idx, pos) = remap(vec![0, 1, 2, 3], 1, 1);
        assert_eq!(idx, vec![0, 1, 2]); // 2→1, 3→2
        assert_eq!(pos, 1); // 已消费 1 项仍为 1，下一次取 idx[1]=1（原 2）
    }

    #[test]
    fn remap_delete_consumed_item() {
        // indices=[0,1,2,3], pos=2（已消费 [0,1]）。删表行 0（已消费）。
        let (idx, pos) = remap(vec![0, 1, 2, 3], 2, 0);
        assert_eq!(idx, vec![0, 1, 2]); // 1→0,2→1,3→2
        assert_eq!(pos, 1); // 已消费项减 1
    }

    #[test]
    fn remap_delete_after_pos() {
        // pos=1（已消费 [0]），删表行 3（未消费）。
        let (idx, pos) = remap(vec![0, 1, 2, 3], 1, 3);
        assert_eq!(idx, vec![0, 1, 2]);
        assert_eq!(pos, 1); // 已消费仍为 1
    }

    #[test]
    fn remap_pos_zero() {
        let (idx, pos) = remap(vec![0, 1, 2], 0, 2);
        assert_eq!(idx, vec![0, 1]);
        assert_eq!(pos, 0);
    }

    #[test]
    fn remap_pos_len() {
        // 全部已消费，删中间项。
        let (idx, pos) = remap(vec![0, 1, 2], 3, 1);
        assert_eq!(idx, vec![0, 1]);
        assert_eq!(pos, 2); // 3-1
    }

    #[test]
    fn remap_delete_only_item() {
        let (idx, pos) = remap(vec![5], 1, 5);
        assert!(idx.is_empty());
        assert_eq!(pos, 0);
    }

    #[test]
    fn remap_not_in_set() {
        // 被删索引不在匹配集合中（该行本就不匹配）：集合内容仅左移，不删项。
        let (idx, pos) = remap(vec![0, 2, 4], 2, 1);
        assert_eq!(idx, vec![0, 1, 3]); // 2→1, 4→3
        assert_eq!(pos, 2); // 已消费 [0,2] 仍为 2 项（均在 out 中）
    }

    #[test]
    fn remap_delete_middle_of_set() {
        // removed=3 在集合中：移除后 5→4、7→6。
        let (idx, pos) = remap(vec![1, 3, 5, 7], 1, 3);
        assert_eq!(idx, vec![1, 4, 6]); // 3 被移除；5→4, 7→6
        assert_eq!(pos, 1); // 已消费 [1] 仍在
    }
}
