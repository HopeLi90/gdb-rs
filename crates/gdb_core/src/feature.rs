//! 要素（`Feature`）：行 + 几何访问。几何存储于表的几何字段（`FieldValue::Geometry`）。

use crate::error::{GdbError, Result};
use crate::geometry::Geometry;
use crate::row::Row;
use crate::value::FieldValue;

/// 一个要素（含几何的行的视图）。
#[derive(Clone)]
pub struct Feature {
    row: Row,
}

impl Feature {
    pub(crate) fn new(row: Row) -> Self {
        Feature { row }
    }

    /// 该要素的 OBJECTID。
    pub fn object_id(&self) -> u64 {
        self.row.object_id()
    }

    /// 按字段索引取值。
    pub fn get(&self, i: usize) -> FieldValue {
        self.row.get(i)
    }

    /// 按字段名取值。
    pub fn get_by_name(&self, name: &str) -> Result<FieldValue> {
        self.row.get_by_name(name)
    }

    /// 按字段索引赋值。
    pub fn set(&self, i: usize, v: FieldValue) {
        self.row.set(i, v);
    }

    /// 按字段名赋值。
    pub fn set_by_name(&self, name: &str, v: FieldValue) -> Result<()> {
        self.row.set_by_name(name, v)
    }

    /// 获取几何（要求该要素类含几何字段）。
    pub fn geometry(&self) -> Result<Geometry> {
        let t = self.row.table.borrow();
        let gi = t
            .schema
            .geometry_index()
            .ok_or_else(|| GdbError::GeometryError("该对象不含几何字段".into()))?;
        match &t.rows[self.row.index][gi] {
            FieldValue::Geometry(g) => Ok(g.clone()),
            FieldValue::Null => Err(GdbError::GeometryError("几何为空".into())),
            other => Err(GdbError::GeometryError(format!(
                "几何字段类型异常: {other:?}"
            ))),
        }
    }

    /// 设置几何（写入表的几何字段）。
    pub fn set_geometry(&self, g: Geometry) -> Result<()> {
        let mut t = self.row.table.borrow_mut();
        let gi = t
            .schema
            .geometry_index()
            .ok_or_else(|| GdbError::GeometryError("该对象不含几何字段".into()))?;
        t.rows[self.row.index][gi] = FieldValue::Geometry(g);
        Ok(())
    }

    /// 写回（同 `Row::store`）。
    pub fn store(&self) {
        self.row.store();
    }

    /// 底层行。
    pub fn row(&self) -> &Row {
        &self.row
    }
}
