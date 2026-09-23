//! 字段类型、字段定义、几何类型与表模式（schema）。

use crate::value::FieldValue;

/// FileGDB 字段类型（对应二进制字段描述中的 type 字节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    Int16,
    Int32,
    Float32,
    Float64,
    String,
    DateTime,
    ObjectId,
    Geometry,
    Binary,
    Raster,
    Uuid,
    Xml,
    Int64,
    Date,
    Time,
    /// 其它未在本文档中展开的字段类型（保留原始编码以便回写）。
    Other(u8),
}

impl FieldType {
    pub fn from_code(c: u8) -> Self {
        match c {
            0 => FieldType::Int16,
            1 => FieldType::Int32,
            2 => FieldType::Float32,
            3 => FieldType::Float64,
            4 => FieldType::String,
            5 => FieldType::DateTime,
            6 => FieldType::ObjectId,
            7 => FieldType::Geometry,
            8 => FieldType::Binary,
            9 => FieldType::Raster,
            10 | 11 => FieldType::Uuid,
            12 => FieldType::Xml,
            13 => FieldType::Int64,
            14 => FieldType::Date,
            15 => FieldType::Time,
            other => FieldType::Other(other),
        }
    }

    pub fn code(self) -> u8 {
        match self {
            FieldType::Int16 => 0,
            FieldType::Int32 => 1,
            FieldType::Float32 => 2,
            FieldType::Float64 => 3,
            FieldType::String => 4,
            FieldType::DateTime => 5,
            FieldType::ObjectId => 6,
            FieldType::Geometry => 7,
            FieldType::Binary => 8,
            FieldType::Raster => 9,
            FieldType::Uuid => 10,
            FieldType::Xml => 12,
            FieldType::Int64 => 13,
            FieldType::Date => 14,
            FieldType::Time => 15,
            FieldType::Other(c) => c,
        }
    }

    /// 行值是否为定长存储（无长度前缀、内联在行中）。
    /// 注意：ObjectId 在行中不存储（由槽位号合成）。
    pub fn is_fixed(self) -> bool {
        matches!(
            self,
            FieldType::Int16
                | FieldType::Int32
                | FieldType::Float32
                | FieldType::Float64
                | FieldType::DateTime
                | FieldType::Date
                | FieldType::Time
                | FieldType::Int64
                | FieldType::Uuid
        )
    }

    /// 定长字段的行内字节数（变长字段返回 0）。
    pub fn fixed_size(self) -> usize {
        match self {
            FieldType::Int16 => 2,
            FieldType::Int32 => 4,
            FieldType::Float32 => 4,
            FieldType::Float64 => 8,
            FieldType::DateTime | FieldType::Date | FieldType::Time => 8,
            FieldType::Int64 => 8,
            FieldType::Uuid => 16,
            _ => 0,
        }
    }
}

/// 几何基础类型（layer_flags 低字节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometryType {
    None,
    Point,
    MultiPoint,
    Polyline,
    Polygon,
    Envelope,
    MultiPatch,
    /// 其它（曲线/3D/混合等）保留原始编码。
    Other(i32),
}

impl GeometryType {
    pub fn from_code(c: i32) -> Self {
        match c {
            0 => GeometryType::None,
            1 => GeometryType::Point,
            2 => GeometryType::MultiPoint,
            3 => GeometryType::Polyline,
            4 => GeometryType::Polygon,
            5 => GeometryType::Envelope,
            9 => GeometryType::MultiPatch,
            other => GeometryType::Other(other),
        }
    }

    pub fn code(self) -> i32 {
        match self {
            GeometryType::None => 0,
            GeometryType::Point => 1,
            GeometryType::MultiPoint => 2,
            GeometryType::Polyline => 3,
            GeometryType::Polygon => 4,
            GeometryType::Envelope => 5,
            GeometryType::MultiPatch => 9,
            GeometryType::Other(c) => c,
        }
    }
}

/// 精度网格参数（几何坐标量化用）。
///
/// 真实坐标 = `i / scale + origin`；写出时 `i = round((coord - origin) * scale)`。
#[derive(Debug, Clone, PartialEq)]
pub struct PrecisionGrid {
    pub xorig: f64,
    pub yorig: f64,
    pub xyscale: f64,
    pub zorig: f64,
    pub zscale: f64,
    pub morig: f64,
    pub mscale: f64,
    /// XY 容差（字段定义中的 xytolerance）。
    pub xytol: f64,
    /// M 容差。
    pub mtol: f64,
    /// Z 容差。
    pub ztol: f64,
    /// 图层范围 (xmin, ymin, xmax, maxy)。
    pub extent: (f64, f64, f64, f64),
    /// 空间索引网格大小（通常 1~3 个）。
    pub grid_sizes: Vec<f64>,
}

impl PrecisionGrid {
    /// 真实 X → 精度网格整数。
    pub fn x_to_grid(&self, x: f64) -> i64 {
        ((x - self.xorig) * self.xyscale).round() as i64
    }
    /// 精度网格整数 → 真实 X。
    pub fn grid_to_x(&self, g: i64) -> f64 {
        g as f64 / self.xyscale + self.xorig
    }
    /// 真实 Y → 精度网格整数。
    pub fn y_to_grid(&self, y: f64) -> i64 {
        ((y - self.yorig) * self.xyscale).round() as i64
    }
    /// 精度网格整数 → 真实 Y。
    pub fn grid_to_y(&self, g: i64) -> f64 {
        g as f64 / self.xyscale + self.yorig
    }
}

impl Default for PrecisionGrid {
    fn default() -> Self {
        // 与 ArcGIS/OGR 默认一致的常用网格（投影坐标系）。
        PrecisionGrid {
            xorig: -2147483647.0,
            yorig: -2147483647.0,
            xyscale: 1e9,
            zorig: -1e9,
            zscale: 1e9,
            morig: -1e9,
            mscale: 1e9,
            xytol: 0.001,
            mtol: 0.001,
            ztol: 0.001,
            extent: (0.0, 0.0, 0.0, 0.0),
            grid_sizes: Vec::new(),
        }
    }
}

/// 单个字段的定义（含名称、类型、可空、长度等）。
#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    pub alias: String,
    pub field_type: FieldType,
    /// 是否可空（flags bit0，0x01）。
    pub nullable: bool,
    /// 是否必填（flags bit1，0x02）。ObjectId 字段恒为 required。
    pub required: bool,
    /// 是否可编辑（flags bit2，0x04）。仅 editable 字段的定义中携带默认值。
    pub editable: bool,
    /// 字符串最大长度（仅 string 类型有意义）。
    pub length: i32,
    /// 几何字段的精度网格（仅 geometry 类型有意义）。
    pub grid: PrecisionGrid,
    /// 几何字段的空间参考 WKT（仅 geometry 类型有意义）。
    pub srs_wkt: String,
    /// 几何字段是否含 Z（gflag bit2）。
    pub geom_has_z: bool,
    /// 几何字段是否含 M（gflag bit1）。
    pub geom_has_m: bool,
    /// 字段默认值（定义中携带时）。
    pub default: Option<FieldValue>,
}

impl FieldDef {
    pub fn new(name: &str, field_type: FieldType) -> Self {
        FieldDef {
            name: name.to_string(),
            alias: String::new(),
            field_type,
            nullable: true,
            required: false,
            editable: true,
            length: 0,
            grid: PrecisionGrid::default(),
            srs_wkt: String::new(),
            geom_has_z: false,
            geom_has_m: false,
            default: None,
        }
    }

    /// 合成字段 flags 字节：bit0=nullable、bit1=required、bit2=editable。
    pub fn flags_byte(&self) -> u8 {
        let mut f = 0u8;
        if self.nullable {
            f |= 0x01;
        }
        if self.required {
            f |= 0x02;
        }
        if self.editable {
            f |= 0x04;
        }
        f
    }
}

/// 表/要素类的完整模式（schema）。
#[derive(Debug, Clone)]
pub struct TableSchema {
    /// 几何类型；None 表示这是无几何的属性表。
    pub geometry_type: GeometryType,
    /// 是否包含 Z 维度。
    pub has_z: bool,
    /// 是否包含 M 维度。
    pub has_m: bool,
    /// 文本是否以 UTF-8 编码（否则 UTF-16）。
    pub string_utf8: bool,
    /// 全部字段（含隐含的 OBJECTID 字段与几何字段）。
    pub fields: Vec<FieldDef>,
}

impl TableSchema {
    /// 返回 OBJECTID 字段的索引（FileGDB 中通常为第 0 个字段）。
    pub fn objectid_index(&self) -> Option<usize> {
        self.fields
            .iter()
            .position(|f| f.field_type == FieldType::ObjectId)
    }

    /// 返回几何字段的索引（无几何则为 None）。
    pub fn geometry_index(&self) -> Option<usize> {
        self.fields
            .iter()
            .position(|f| f.field_type == FieldType::Geometry)
    }

    /// 按字段名查找索引。
    pub fn field_index(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|f| f.name == name)
    }

    /// 可空字段个数（用于 null 位图字节数计算）。
    pub fn nullable_count(&self) -> usize {
        self.fields.iter().filter(|f| f.nullable).count()
    }
}
