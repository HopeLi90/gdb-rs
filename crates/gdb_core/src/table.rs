//! 表（.gdbtable + .gdbtablx）的解析、内存模型与回写。
//!
//! 已对照真实 ArcGIS 数据与 GDAL OpenFileGDB 实现实证的格式：
//!
//! 行 blob 结构：
//! ```text
//! [int32 row_len]  // 负值（最高位为 1）表示已删除记录
//! [null 位图]      // 仅当表存在可空字段时：ceil(可空字段数/8) 字节，
//!                  //  bit k = 第 k 个可空字段（按字段顺序）为 NULL
//! [字段值流]       // 按字段顺序（跳过 NULL 字段）：
//! ```
//! - ObjectId 字段**不存储**：其值 = 行槽位号 + 1（由 .gdbtablx 中的槽位合成）。
//! - String/Xml：`varuint 字节长 + 数据`（UTF-8 或 UTF-16，见 layer_flags&0x100）。
//! - Geometry/Binary/Raster：`varuint 字节长 + 数据`。
//! - Guid/GlobalId：固定 16 字节原始数据（无长度前缀）。
//! - Int16/Int32/Float32/Float64/DateTime/Date/Time/Int64：定长小端内联。
//!
//! `.gdbtablx` 头 16 字节：
//! ```text
//! [i32 version][i32 n1024blocks][i32 nrows][i32 offset_size]
//! ```
//! 行偏移自字节 16 起，每条 `offset_size` 字节小端；0 表示空槽（已删除行）。
//! `nrows` 为槽位总数（可大于有效行数），`offset_size` 直接从头部读取。

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{GdbError, Result};
use crate::field::{FieldDef, FieldType, GeometryType, TableSchema};
use crate::geometry::encode_geometry_blob;
use crate::io::{Reader, write_varuint};
use crate::value::{DateTime, FieldValue, Uuid};

/// 表的完整内存表示（一个 .gdbtable 文件）。
#[derive(Debug, Clone)]
pub struct Table {
    /// 目录（.gdb 文件夹）路径。
    pub directory: PathBuf,
    /// 文件编号（aNNNNNNNN 的 NNNNNNNN）。
    pub file_id: u32,
    /// 格式版本（3 或 4）。
    pub version: u32,
    /// 模式（schema）。
    pub schema: TableSchema,
    /// 全部行的值（每行一个 Vec，按字段索引对齐）。
    pub rows: Vec<Vec<FieldValue>>,
    /// 每行对应的 .gdbtablx 槽位号（与 rows 一一对应）。
    /// OID = 槽位号 + 1；保留原槽位可保证更新回写后 OID 不变。
    pub row_slots: Vec<u64>,
    /// 槽位总数（含空槽；新建表为已追加行数）。
    pub slot_count: u64,
}

impl Table {
    /// 打开并解析一个 .gdbtable（配合同名的 .gdbtablx）。
    pub fn open(directory: &Path, file_id: u32) -> Result<Table> {
        let table_path = directory.join(format!("a{:08x}.gdbtable", file_id));
        let data = fs::read(&table_path)?;
        let schema = parse_schema(&data)?;
        let version = schema_version(&data)?;
        let slots = read_row_slots(directory, file_id)?;

        let mut rows = Vec::with_capacity(slots.len());
        let mut row_slots = Vec::with_capacity(slots.len());
        let mut slot_count = 0u64;
        for (slot, offset) in slots {
            slot_count = slot_count.max(slot + 1);
            if offset == 0 {
                continue; // 空槽（已删除行）
            }
            let row = decode_row(&data, offset, slot, &schema)?;
            rows.push(row);
            row_slots.push(slot);
        }

        Ok(Table {
            directory: directory.to_path_buf(),
            file_id,
            version,
            schema,
            rows,
            row_slots,
            slot_count,
        })
    }

    /// 仅含 OBJECTID 字段的空模式骨架（用于新建对象时扩展）。
    pub fn empty_schema() -> TableSchema {
        let mut fields = Vec::new();
        let mut oid = crate::field::FieldDef::new("OBJECTID", crate::field::FieldType::ObjectId);
        // ObjectId 恒为 required 且不可编辑（flags = 0x02，GDAL 强制校验）。
        oid.nullable = false;
        oid.required = true;
        oid.editable = false;
        fields.push(oid);
        TableSchema {
            geometry_type: crate::field::GeometryType::None,
            has_z: false,
            has_m: false,
            string_utf8: true,
            fields,
        }
    }

    /// 创建空表（用于新建/写入）。
    pub fn new(directory: &Path, file_id: u32, version: u32, schema: TableSchema) -> Table {
        Table {
            directory: directory.to_path_buf(),
            file_id,
            version,
            schema,
            rows: Vec::new(),
            row_slots: Vec::new(),
            slot_count: 0,
        }
    }

    /// 追加一行（需提供与 schema 字段数一致的值）。
    pub fn add_row(&mut self, values: Vec<FieldValue>) -> Result<()> {
        if values.len() != self.schema.fields.len() {
            return Err(GdbError::Format(format!(
                "行字段数 {} 与 schema {} 不一致",
                values.len(),
                self.schema.fields.len()
            )));
        }
        // 追加到下一个空槽（保持 OID = 槽位 + 1 的语义）。
        self.row_slots.push(self.slot_count);
        self.slot_count += 1;
        self.rows.push(values);
        Ok(())
    }

    /// 按 OBJECTID 查找行索引。
    pub fn row_index_by_oid(&self, oid: u64) -> Option<usize> {
        let oi = self.schema.objectid_index()?;
        self.rows
            .iter()
            .position(|r| matches!(r[oi], FieldValue::ObjectId(v) if v == oid))
    }

    /// 序列化并写回磁盘（.gdbtable 与 .gdbtablx）。
    pub fn flush(&self) -> Result<()> {
        let (table_bytes, tablx_bytes) = self.serialize()?;
        let table_path = self.directory.join(format!("a{:08x}.gdbtable", self.file_id));
        let tablx_path = self.directory.join(format!("a{:08x}.gdbtablx", self.file_id));
        fs::write(&table_path, &table_bytes)?;
        fs::write(&tablx_path, &tablx_bytes)?;
        Ok(())
    }

    /// 计算序列化字节（不写盘），供测试与 flush 复用。
    pub fn serialize(&self) -> Result<(Vec<u8>, Vec<u8>)> {
        serialize_table(self)
    }
}

// ---------------------------------------------------------------------------
// 头部解析
// ---------------------------------------------------------------------------

fn schema_version(data: &[u8]) -> Result<u32> {
    if data.len() < 4 {
        return Err(GdbError::Format("文件过短，无法读取版本".into()));
    }
    let v = i32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if v != 3 && v != 4 {
        return Err(GdbError::Format(format!("不支持的 .gdbtable 版本 {v}")));
    }
    Ok(v as u32)
}

fn read_field_desc_offset(data: &[u8]) -> u64 {
    u64::from_le_bytes(data[32..40].try_into().unwrap())
}

// ---------------------------------------------------------------------------
// 字段描述段解析
// ---------------------------------------------------------------------------

fn parse_schema(data: &[u8]) -> Result<TableSchema> {
    let desc_off = read_field_desc_offset(data) as usize;
    if desc_off + 4 > data.len() {
        return Err(GdbError::Format("字段描述段偏移越界".into()));
    }
    let mut r = Reader::new(&data[desc_off..]);
    let _section_len = r.u32()?; // 描述段长度（不含本 4 字节）
    let _fdesc_version = r.u32()?;
    let layer_flags = r.u32()?;
    // 次级头 4 字节布局（与 GDAL abyHeader[8..12] 对应）：
    // byte8 = 表几何类型；byte9 bit0 = 文本为 UTF-8；byte11 bit7/bit6 = 表含 Z/M。
    let geom_base = (layer_flags & 0xFF) as i32;
    let has_z = (layer_flags & 0x8000_0000) != 0;
    let has_m = (layer_flags & 0x4000_0000) != 0;
    let string_utf8 = (layer_flags & 0x100) != 0;
    let geometry_type = GeometryType::from_code(geom_base);
    let nfields = r.u16()? as usize;

    let mut fields = Vec::with_capacity(nfields);
    for _ in 0..nfields {
        fields.push(read_field_def(&mut r, has_z, has_m)?);
    }

    Ok(TableSchema {
        geometry_type,
        has_z,
        has_m,
        string_utf8,
        fields,
    })
}

/// 字段 flags 位定义（经真实数据 + GDAL `filegdbtable.h` 双重验证）：
/// bit0(0x01)=nullable、bit1(0x02)=required、bit2(0x04)=editable。
const NULLABLE: u8 = 0x01;
const REQUIRED: u8 = 0x02;
const EDITABLE: u8 = 0x04;

fn read_field_def(
    r: &mut Reader<'_>,
    tab_has_z: bool,
    tab_has_m: bool,
) -> Result<FieldDef> {
    let name_len = r.u8()? as usize;
    let name = r.utf16(name_len)?;
    let alias_len = r.u8()? as usize;
    let alias = if alias_len > 0 {
        r.utf16(alias_len)?
    } else {
        String::new()
    };
    let ftype = FieldType::from_code(r.u8()?);
    let mut def = FieldDef::new(&name, ftype);
    def.alias = alias;

    match ftype {
        FieldType::String => {
            // [i32 maxWidth][u8 flags][varuint defLen][editable 时: defLen 字节默认值]
            def.length = r.i32()?;
            let flags = r.u8()?;
            def.nullable = flags & NULLABLE != 0;
            def.required = flags & REQUIRED != 0;
            def.editable = flags & EDITABLE != 0;
            let def_len = r.varuint()? as usize;
            if def.editable && def_len > 0 {
                let b = r.bytes(def_len)?;
                let s = if def_len >= 2 && b.len() % 2 == 0 && looks_like_utf16(b) {
                    let units: Vec<u16> =
                        b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                    String::from_utf16_lossy(&units)
                } else {
                    String::from_utf8_lossy(b).to_string()
                };
                def.default = Some(FieldValue::Text(s));
            }
        }
        FieldType::ObjectId | FieldType::Binary | FieldType::Raster | FieldType::Uuid
        | FieldType::Xml => {
            // [u8 ?][u8 flags] 共 2 字节
            let _w = r.u8()?;
            let flags = r.u8()?;
            def.nullable = flags & NULLABLE != 0;
            def.required = flags & REQUIRED != 0;
            def.editable = flags & EDITABLE != 0;
            if ftype == FieldType::ObjectId {
                // GDAL 校验 ObjectId 的 flags 恒为 MASK_REQUIRED(0x02)。
                def.nullable = false;
                def.required = true;
                def.editable = false;
            }
        }
        FieldType::Geometry => {
            // [u8 0][u8 flags][u16 wktByteLen][WKT UTF16LE]
            // [u8 gflags][f64 xorig][f64 yorig][f64 xyscale]
            // [gflags&2: f64 morig, f64 mscale][gflags&4: f64 zorig, f64 zscale]
            // [f64 xytol][gflags&2: f64 mtol][gflags&4: f64 ztol]
            // [f64 xmin][f64 ymin][f64 xmax][f64 ymax]
            // [表级Z: f64 zmin, f64 zmax][表级M: f64 mmin, f64 mmax]
            // [u8 0][u32 ngrid][ngrid × f64]
            //
            // 注意：字段 flags（第 2 字节）与 gflags（WKT 后）是两个不同字节；
            // gflags 仅表示 M/Z 原点-比例-容差参数组是否存在（真实文件恒 0x07），
            // 几何是否含 Z/M 由表级标志（次级头 byte11）决定。
            let _magic = r.u8()?;
            let flags = r.u8()?;
            def.nullable = flags & NULLABLE != 0;
            def.required = flags & REQUIRED != 0;
            def.editable = flags & EDITABLE != 0;
            let wkt_len = r.u16()? as usize;
            if wkt_len > 0 {
                let b = r.bytes(wkt_len)?;
                let units: Vec<u16> =
                    b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                def.srs_wkt = String::from_utf16_lossy(&units);
            }
            let gflags = r.u8()?;
            let m_params = gflags & 0x02 != 0;
            let z_params = gflags & 0x04 != 0;
            def.geom_has_m = tab_has_m;
            def.geom_has_z = tab_has_z;
            def.grid.xorig = r.f64()?;
            def.grid.yorig = r.f64()?;
            def.grid.xyscale = r.f64()?;
            if m_params {
                def.grid.morig = r.f64()?;
                def.grid.mscale = r.f64()?;
            }
            if z_params {
                def.grid.zorig = r.f64()?;
                def.grid.zscale = r.f64()?;
            }
            def.grid.xytol = r.f64()?;
            if m_params {
                def.grid.mtol = r.f64()?;
            }
            if z_params {
                def.grid.ztol = r.f64()?;
            }
            let xmin = r.f64()?;
            let ymin = r.f64()?;
            let xmax = r.f64()?;
            let ymax = r.f64()?;
            def.grid.extent = (xmin, ymin, xmax, ymax);
            // 表级含 Z/M 时，外接矩形后跟随 Z/M 范围（各 2 个 double）。
            if tab_has_z {
                let _zmin = r.f64()?;
                let _zmax = r.f64()?;
            }
            if tab_has_m {
                let _mmin = r.f64()?;
                let _mmax = r.f64()?;
            }
            let _zero = r.u8()?;
            let ngrid = r.u32()? as usize;
            let mut grids = Vec::with_capacity(ngrid);
            for _ in 0..ngrid {
                grids.push(r.f64()?);
            }
            def.grid.grid_sizes = grids;
        }
        // 数值/日期型：[u8 width][u8 flags][u8 defLen][editable 时: defLen 字节默认值]
        _ => {
            let _width = r.u8()?;
            let flags = r.u8()?;
            def.nullable = flags & NULLABLE != 0;
            def.required = flags & REQUIRED != 0;
            def.editable = flags & EDITABLE != 0;
            let def_len = r.u8()? as usize;
            if def.editable && def_len > 0 {
                let b = r.bytes(def_len)?;
                def.default = parse_fixed_default(b, ftype);
            }
        }
    }
    Ok(def)
}

/// 判断字节串是否更像 UTF-16LE 文本（用于默认值启发式解码）。
fn looks_like_utf16(b: &[u8]) -> bool {
    if b.len() < 2 {
        return false;
    }
    // UTF-16LE ASCII 文本的奇数位（低字节）非零、偶数位（高字节）常为 0。
    let mut odd_zero = 0usize;
    for (i, c) in b.iter().enumerate() {
        if i % 2 == 1 && *c == 0 {
            odd_zero += 1;
        }
    }
    odd_zero * 2 >= b.len() / 2
}

/// 按字段类型解析定长默认值。
fn parse_fixed_default(b: &[u8], ftype: FieldType) -> Option<FieldValue> {
    match ftype {
        FieldType::Int16 if b.len() == 2 => {
            Some(FieldValue::Int16(i16::from_le_bytes([b[0], b[1]])))
        }
        FieldType::Int32 if b.len() == 4 => Some(FieldValue::Int32(i32::from_le_bytes(
            b.try_into().unwrap(),
        ))),
        FieldType::Float32 if b.len() == 4 => Some(FieldValue::Float(f32::from_le_bytes(
            b.try_into().unwrap(),
        ))),
        FieldType::Float64 if b.len() == 8 => Some(FieldValue::Double(f64::from_le_bytes(
            b.try_into().unwrap(),
        ))),
        FieldType::DateTime | FieldType::Date | FieldType::Time if b.len() == 8 => {
            Some(FieldValue::DateTime(DateTime::from_ole(f64::from_le_bytes(
                b.try_into().unwrap(),
            ))))
        }
        FieldType::Int64 if b.len() == 8 => Some(FieldValue::Int64(i64::from_le_bytes(
            b.try_into().unwrap(),
        ))),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 行偏移索引（.gdbtablx）
// ---------------------------------------------------------------------------

/// 读取 .gdbtablx 的行槽位：返回 (槽位号, 文件偏移) 列表（含空槽 offset=0）。
fn read_row_slots(directory: &Path, file_id: u32) -> Result<Vec<(u64, u64)>> {
    let tablx_path = directory.join(format!("a{:08x}.gdbtablx", file_id));
    let idx = fs::read(&tablx_path)?;
    if idx.len() < 16 {
        return Err(GdbError::Format(".gdbtablx 过短".into()));
    }
    // 头部 16 字节：version(i32)@0, n1024blocks(i32)@4, nrows(i32)@8, offset_size(i32)@12
    let _idx_version = i32::from_le_bytes([idx[0], idx[1], idx[2], idx[3]]);
    let nrows = i32::from_le_bytes([idx[8], idx[9], idx[10], idx[11]]);
    let offset_size = i32::from_le_bytes([idx[12], idx[13], idx[14], idx[15]]);
    if nrows < 0 || offset_size <= 0 || offset_size > 8 {
        return Err(GdbError::Format(format!(
            ".gdbtablx 头部非法: nrows={nrows} offset_size={offset_size}"
        )));
    }
    let width = offset_size as usize;
    let mut slots = Vec::with_capacity(nrows as usize);
    let mut pos = 16usize;
    for slot in 0..nrows as u64 {
        if pos + width > idx.len() {
            // 尾部不足时按可用数据截断（个别生成器会预分配页导致文件偏大，反向亦然）
            break;
        }
        slots.push((slot, read_uint_le(&idx[pos..pos + width], width)));
        pos += width;
    }
    Ok(slots)
}

fn read_uint_le(buf: &[u8], width: usize) -> u64 {
    let mut v: u64 = 0;
    (0..width).for_each(|i| {
        v |= (buf[i] as u64) << (8 * i);
    });
    v
}

// ---------------------------------------------------------------------------
// 行解码
// ---------------------------------------------------------------------------

fn decode_row(
    data: &[u8],
    offset: u64,
    slot: u64,
    schema: &TableSchema,
) -> Result<Vec<FieldValue>> {
    let off = offset as usize;
    if off + 4 > data.len() {
        return Err(GdbError::Format("行偏移越界".into()));
    }
    let mut r = Reader::new(&data[off..]);
    let row_len = r.i32()?;
    if row_len < 0 {
        return Err(GdbError::Format("行偏移指向已删除记录".into()));
    }
    let row_len = row_len as usize;
    if off + 4 + row_len > data.len() {
        return Err(GdbError::Format("行长度越界".into()));
    }
    let end = off + 4 + row_len;

    // null 位图（仅当存在可空字段时）
    let nullable_count = schema.nullable_count();
    let null_bytes = nullable_count.div_ceil(8);
    let mut null_bits: Vec<u8> = Vec::with_capacity(null_bytes);
    for _ in 0..null_bytes {
        null_bits.push(r.u8()?);
    }

    let mut nullable_seen = 0usize;
    let mut values = Vec::with_capacity(schema.fields.len());
    for fdef in &schema.fields {
        if fdef.field_type == FieldType::ObjectId {
            // ObjectId 不存储在行内，由槽位号合成（从 1 开始）。
            values.push(FieldValue::ObjectId(slot + 1));
            continue;
        }

        let is_null = if fdef.nullable {
            let byte = null_bits[nullable_seen / 8];
            let bit = (byte >> (nullable_seen % 8)) & 1;
            nullable_seen += 1;
            bit != 0
        } else {
            false
        };
        if is_null {
            values.push(FieldValue::Null);
            continue;
        }

        // 值流式读取；r.pos 相对行起点，绝对位置 = off + r.pos。
        let field_abs = off + r.pos;
        let v = decode_field_value(data, field_abs, fdef, schema)?;
        // 推进游标到该字段值末尾。
        advance_reader(&mut r, &data[off..end], field_abs - off, fdef)?;
        values.push(v);
    }
    Ok(values)
}

/// 将读取游标推进到当前字段值的末尾（按类型计算 consumed 长度）。
fn advance_reader(
    r: &mut Reader<'_>,
    row: &[u8],
    field_rel: usize,
    fdef: &FieldDef,
) -> Result<()> {
    let consumed = match fdef.field_type {
        FieldType::Int16 => 2,
        FieldType::Int32 => 4,
        FieldType::Float32 => 4,
        FieldType::Float64 => 8,
        FieldType::DateTime | FieldType::Date | FieldType::Time => 8,
        FieldType::Int64 => 8,
        FieldType::Uuid => 16,
        FieldType::ObjectId => 0, // 不存储于行内（decode_row 中已提前返回）
        FieldType::String
        | FieldType::Xml
        | FieldType::Geometry
        | FieldType::Binary
        | FieldType::Raster => {
            // varuint 长度前缀 + 数据
            let mut rr = Reader::new(&row[field_rel..]);
            let len = rr.varuint()? as usize;
            rr.pos + len
        }
        FieldType::Other(_) => row.len() - field_rel, // 尽力消费到行尾
    };
    r.pos = field_rel + consumed;
    if r.pos > row.len() {
        return Err(GdbError::Format(format!(
            "字段 {} 值越界 (pos={}, row_len={})",
            fdef.name,
            r.pos,
            row.len()
        )));
    }
    Ok(())
}

fn decode_field_value(
    data: &[u8],
    field_abs: usize,
    fdef: &FieldDef,
    schema: &TableSchema,
) -> Result<FieldValue> {
    let mut r = Reader::new(&data[field_abs..]);
    match fdef.field_type {
        FieldType::Int16 => Ok(FieldValue::Int16(r.i16()?)),
        FieldType::Int32 => Ok(FieldValue::Int32(r.i32()?)),
        FieldType::Float32 => Ok(FieldValue::Float(r.f32()?)),
        FieldType::Float64 => Ok(FieldValue::Double(r.f64()?)),
        FieldType::DateTime | FieldType::Date | FieldType::Time => {
            Ok(FieldValue::DateTime(DateTime::from_ole(r.f64()?)))
        }
        FieldType::Int64 => Ok(FieldValue::Int64(r.i64()?)),
        FieldType::Uuid => {
            let b = r.bytes(16)?;
            let mut arr = [0u8; 16];
            arr.copy_from_slice(b);
            Ok(FieldValue::Uuid(Uuid::from_bytes(arr)))
        }
        FieldType::String | FieldType::Xml => {
            let len = r.varuint()? as usize;
            let bytes = r.bytes(len)?;
            if schema.string_utf8 {
                let s = String::from_utf8_lossy(bytes).to_string();
                if fdef.field_type == FieldType::String {
                    Ok(FieldValue::Text(s))
                } else {
                    Ok(FieldValue::Xml(s))
                }
            } else {
                let units: Vec<u16> = bytes
                    .as_chunks::<2>().0.iter()
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                let s = String::from_utf16_lossy(&units);
                if fdef.field_type == FieldType::String {
                    Ok(FieldValue::Text(s))
                } else {
                    Ok(FieldValue::Xml(s))
                }
            }
        }
        FieldType::Geometry => {
            // 字段值 = varuint blob 长 + blob（decode_geometry_blob 从 blob 起解码）
            let blob_len = r.varuint()? as usize;
            let blob = r.bytes(blob_len)?;
            let geom = crate::geometry::decode_geometry_blob(blob, &fdef.grid)?;
            Ok(FieldValue::Geometry(geom))
        }
        FieldType::Binary | FieldType::Raster => {
            let len = r.varuint()? as usize;
            let bytes = r.bytes(len)?.to_vec();
            Ok(FieldValue::Binary(bytes))
        }
        FieldType::ObjectId => Ok(FieldValue::ObjectId(0)), // 由调用方合成
        FieldType::Other(_) => Ok(FieldValue::Binary(Vec::new())),
    }
}

// ---------------------------------------------------------------------------
// 序列化（回写）
// ---------------------------------------------------------------------------

fn serialize_table(table: &Table) -> Result<(Vec<u8>, Vec<u8>)> {
    let schema = &table.schema;
    let nfields = schema.fields.len();

    // 1) 字段描述段字节
    let mut fdesc = Vec::new();
    // 占位 4 字节 section_len（稍后回填）
    fdesc.extend_from_slice(&0u32.to_le_bytes());
    fdesc.extend_from_slice(&(if table.version == 4 { 6u32 } else { 4u32 }).to_le_bytes());
    let mut layer_flags: u32 = schema.geometry_type.code() as u32 & 0xFF;
    if schema.has_z {
        layer_flags |= 0x8000_0000;
    }
    if schema.has_m {
        layer_flags |= 0x4000_0000;
    }
    if schema.string_utf8 {
        layer_flags |= 0x100;
    }
    fdesc.extend_from_slice(&layer_flags.to_le_bytes());
    fdesc.extend_from_slice(&(nfields as u16).to_le_bytes());
    for fdef in &schema.fields {
        write_field_def(&mut fdesc, fdef, schema);
    }
    let section_len = (fdesc.len() - 4) as u32;
    fdesc[0..4].copy_from_slice(&section_len.to_le_bytes());

    // 2) 逐行编码
    let mut row_blobs: Vec<Vec<u8>> = Vec::with_capacity(table.rows.len());
    let mut largest = 0usize;
    for row in &table.rows {
        let blob = encode_row(row, schema);
        largest = largest.max(blob.len());
        row_blobs.push(blob);
    }

    // 3) 组装 .gdbtable
    let field_desc_offset: u64 = 40;
    let mut table_bytes: Vec<u8> = vec![0; 40];
    // version
    table_bytes[0..4].copy_from_slice(&(table.version as i32).to_le_bytes());
    if table.version == 3 {
        table_bytes[4..8].copy_from_slice(&(table.rows.len() as u32).to_le_bytes());
    } else {
        table_bytes[16..24].copy_from_slice(&(table.rows.len() as u64).to_le_bytes());
    }
    table_bytes[8..12].copy_from_slice(&(largest as u32).to_le_bytes());
    // [12..16]：真实生成器（v3/v4 表）恒写 5。
    table_bytes[12..16].copy_from_slice(&5i32.to_le_bytes());
    // field_desc_offset @32
    table_bytes[32..40].copy_from_slice(&field_desc_offset.to_le_bytes());
    // 字段描述段
    table_bytes.extend_from_slice(&fdesc);
    // 行
    let mut row_offsets = Vec::with_capacity(table.rows.len());
    for blob in &row_blobs {
        row_offsets.push(table_bytes.len() as u64);
        table_bytes.extend_from_slice(blob);
    }
    let file_size = table_bytes.len() as u64;
    table_bytes[24..32].copy_from_slice(&file_size.to_le_bytes());

    // 4) .gdbtablx：[i32 version=3][i32 n1024blocks][i32 nrows][i32 offset_size]
    // 行写回其**原槽位**（row_slots），空槽保持 offset=0，
    // 以保证更新回写后 OBJECTID（= 槽位+1）不变。
    let slots: Vec<u64> = if table.row_slots.len() == table.rows.len() && !table.rows.is_empty() {
        table.row_slots.clone()
    } else {
        (0..table.rows.len() as u64).collect()
    };
    let slot_count = slots
        .iter()
        .copied()
        .max()
        .map(|m| m + 1)
        .unwrap_or(0)
        .max(table.rows.len() as u64);
    let mut slot_offsets = vec![0u64; slot_count as usize];
    for (slot, off) in slots.iter().zip(&row_offsets) {
        slot_offsets[*slot as usize] = *off;
    }
    let width = offset_byte_width_for(&slot_offsets);
    let n1024blocks = (slot_count as usize).div_ceil(1024);
    let mut tablx = Vec::new();
    tablx.extend_from_slice(&3i32.to_le_bytes());
    tablx.extend_from_slice(&(n1024blocks as i32).to_le_bytes());
    tablx.extend_from_slice(&(slot_count as i32).to_le_bytes());
    tablx.extend_from_slice(&(width as i32).to_le_bytes());
    for off in &slot_offsets {
        for b in 0..width {
            tablx.push((off >> (8 * b)) as u8);
        }
    }

    Ok((table_bytes, tablx))
}

/// 依最大行偏移选择偏移字节宽（与 ESRI 生成器一致的最小必要宽度）。
fn offset_byte_width_for(offsets: &[u64]) -> usize {
    let max = offsets.iter().copied().max().unwrap_or(0);
    if max <= 0xFFFF_FFFF {
        4
    } else if max <= 0x00FF_FFFF_FFFF {
        5
    } else {
        6
    }
}

fn write_field_def(out: &mut Vec<u8>, fdef: &FieldDef, schema: &TableSchema) {
    use crate::io::encode_utf16;
    let name_u16 = encode_utf16(&fdef.name);
    out.push((name_u16.len() / 2) as u8);
    out.extend_from_slice(&name_u16);
    let alias_u16 = encode_utf16(&fdef.alias);
    out.push((alias_u16.len() / 2) as u8);
    if !alias_u16.is_empty() {
        out.extend_from_slice(&alias_u16);
    }
    out.push(fdef.field_type.code());

    let flags = fdef.flags_byte();

    match fdef.field_type {
        FieldType::String => {
            // [i32 maxWidth][u8 flags][varuint defLen][editable 时: 默认值]
            out.extend_from_slice(&fdef.length.to_le_bytes());
            out.push(flags);
            match (&fdef.default, fdef.editable) {
                (Some(FieldValue::Text(s)), true) => {
                    let b = s.as_bytes();
                    write_varuint(out, b.len() as u64);
                    out.extend_from_slice(b);
                }
                _ => write_varuint(out, 0),
            }
        }
        FieldType::Geometry => {
            // [u8 0][u8 flags][u16 wktByteLen][WKT UTF16][u8 gflags]
            // [xorig][yorig][xyscale][(M: morig,mscale)][(Z: zorig,zscale)]
            // [xytol][(M: mtol)][(Z: ztol)][xmin,ymin,xmax,ymax]
            // [(表级Z: zmin,zmax)][(表级M: mmin,mmax)][u8 0][u32 ngrid][grids]
            out.push(0u8);
            out.push(flags);
            let wkt = encode_utf16(&fdef.srs_wkt);
            out.extend_from_slice(&(wkt.len() as u16).to_le_bytes());
            out.extend_from_slice(&wkt);
            let has_m = fdef.geom_has_m || schema.has_m;
            let has_z = fdef.geom_has_z || schema.has_z;
            // gflags：bit0 恒置 1（真实生成器行为），bit1=含 M 参数组，bit2=含 Z 参数组。
            let mut gflags: u8 = 0x01;
            if has_m {
                gflags |= 0x02;
            }
            if has_z {
                gflags |= 0x04;
            }
            out.push(gflags);
            let g = &fdef.grid;
            out.extend_from_slice(&g.xorig.to_le_bytes());
            out.extend_from_slice(&g.yorig.to_le_bytes());
            out.extend_from_slice(&g.xyscale.to_le_bytes());
            if has_m {
                out.extend_from_slice(&g.morig.to_le_bytes());
                out.extend_from_slice(&g.mscale.to_le_bytes());
            }
            if has_z {
                out.extend_from_slice(&g.zorig.to_le_bytes());
                out.extend_from_slice(&g.zscale.to_le_bytes());
            }
            out.extend_from_slice(&g.xytol.to_le_bytes());
            if has_m {
                out.extend_from_slice(&g.mtol.to_le_bytes());
            }
            if has_z {
                out.extend_from_slice(&g.ztol.to_le_bytes());
            }
            let (xmin, ymin, xmax, ymax) = g.extent;
            out.extend_from_slice(&xmin.to_le_bytes());
            out.extend_from_slice(&ymin.to_le_bytes());
            out.extend_from_slice(&xmax.to_le_bytes());
            out.extend_from_slice(&ymax.to_le_bytes());
            if has_z {
                out.extend_from_slice(&0f64.to_le_bytes());
                out.extend_from_slice(&0f64.to_le_bytes());
            }
            if has_m {
                out.extend_from_slice(&0f64.to_le_bytes());
                out.extend_from_slice(&0f64.to_le_bytes());
            }
            out.push(0u8);
            out.extend_from_slice(&(g.grid_sizes.len() as u32).to_le_bytes());
            for gs in &g.grid_sizes {
                out.extend_from_slice(&gs.to_le_bytes());
            }
        }
        FieldType::ObjectId | FieldType::Binary | FieldType::Raster | FieldType::Uuid
        | FieldType::Xml => {
            // [u8 width][u8 flags]
            let w: u8 = match fdef.field_type {
                FieldType::ObjectId => 4,
                FieldType::Uuid => 0x26,
                _ => 0,
            };
            out.push(w);
            out.push(flags);
        }
        _ => {
            // 数值/日期型：[u8 width][u8 flags][u8 defLen][editable 时: 默认值]
            let w: u8 = fdef.field_type.fixed_size() as u8;
            out.push(w);
            out.push(flags);
            match (&fdef.default, fdef.editable) {
                (Some(v), true) => {
                    let b = encode_fixed_default(v);
                    out.push(b.len() as u8);
                    out.extend_from_slice(&b);
                }
                _ => out.push(0u8),
            }
        }
    }
}

/// 将默认值编码为字段定义中的定长字节（小端）。
fn encode_fixed_default(v: &FieldValue) -> Vec<u8> {
    match v {
        FieldValue::Int16(x) => x.to_le_bytes().to_vec(),
        FieldValue::Int32(x) => x.to_le_bytes().to_vec(),
        FieldValue::Float(x) => x.to_le_bytes().to_vec(),
        FieldValue::Double(x) => x.to_le_bytes().to_vec(),
        FieldValue::Int64(x) => x.to_le_bytes().to_vec(),
        FieldValue::DateTime(dt) => dt.to_ole().to_le_bytes().to_vec(),
        _ => Vec::new(),
    }
}

fn encode_row(row: &[FieldValue], schema: &TableSchema) -> Vec<u8> {
    // 新布局：[i32 row_len][null 位图][按字段顺序的值流]
    let nb = schema.nullable_count().div_ceil(8);
    let mut nullbits = vec![0u8; nb];
    let mut nullable_seen = 0usize;
    let mut values = Vec::new();
    for (i, fdef) in schema.fields.iter().enumerate() {
        if fdef.field_type == FieldType::ObjectId {
            continue; // 不存储
        }
        if fdef.nullable {
            if matches!(row[i], FieldValue::Null) {
                nullbits[nullable_seen / 8] |= 1 << (nullable_seen % 8);
            }
            nullable_seen += 1;
        }
        if fdef.nullable && matches!(row[i], FieldValue::Null) {
            continue; // NULL 字段无值
        }
        encode_field_value(&mut values, &row[i], fdef, schema);
    }

    let mut out = Vec::with_capacity(4 + nb + values.len());
    // row_len = null 位图 + 值流的总字节数（不含自身的 i32 前缀）。
    out.extend_from_slice(&((nb + values.len()) as u32).to_le_bytes());
    out.extend_from_slice(&nullbits);
    out.extend_from_slice(&values);
    out
}

fn encode_field_value(
    out: &mut Vec<u8>,
    v: &FieldValue,
    fdef: &FieldDef,
    schema: &TableSchema,
) {
    use crate::io::encode_utf16;
    match (fdef.field_type, v) {
        (FieldType::Int16, FieldValue::Int16(x)) => out.extend_from_slice(&x.to_le_bytes()),
        (FieldType::Int32, FieldValue::Int32(x)) => out.extend_from_slice(&x.to_le_bytes()),
        (FieldType::Float32, FieldValue::Float(x)) => out.extend_from_slice(&x.to_le_bytes()),
        (FieldType::Float64, FieldValue::Double(x)) => out.extend_from_slice(&x.to_le_bytes()),
        (FieldType::Int64, FieldValue::Int64(x)) => out.extend_from_slice(&x.to_le_bytes()),
        (FieldType::DateTime, FieldValue::DateTime(dt))
        | (FieldType::Date, FieldValue::DateTime(dt))
        | (FieldType::Time, FieldValue::DateTime(dt)) => {
            out.extend_from_slice(&dt.to_ole().to_le_bytes());
        }
        (FieldType::Uuid, FieldValue::Uuid(u)) => out.extend_from_slice(&u.0),
        (FieldType::Uuid, FieldValue::Text(s)) => {
            // 允许以文本形式赋值 UUID（取前 16 字节十六进制）
            let mut arr = [0u8; 16];
            let hex: Vec<u8> = s.chars().filter(|c| c.is_ascii_hexdigit()).map(|c| c as u8).collect();
            for k in 0..16 {
                if k * 2 + 1 < hex.len() {
                    let hi = (hex[k * 2] as char).to_digit(16).unwrap_or(0) as u8;
                    let lo = (hex[k * 2 + 1] as char).to_digit(16).unwrap_or(0) as u8;
                    arr[k] = hi << 4 | lo;
                }
            }
            out.extend_from_slice(&arr);
        }
        (FieldType::String, FieldValue::Text(s)) => {
            if schema.string_utf8 {
                let b = s.as_bytes();
                write_varuint(out, b.len() as u64);
                out.extend_from_slice(b);
            } else {
                let b = encode_utf16(s);
                write_varuint(out, b.len() as u64);
                out.extend_from_slice(&b);
            }
        }
        (FieldType::Xml, FieldValue::Xml(s)) | (FieldType::String, FieldValue::Xml(s)) => {
            if schema.string_utf8 {
                let b = s.as_bytes();
                write_varuint(out, b.len() as u64);
                out.extend_from_slice(b);
            } else {
                let b = encode_utf16(s);
                write_varuint(out, b.len() as u64);
                out.extend_from_slice(&b);
            }
        }
        (FieldType::Binary, FieldValue::Binary(b)) | (FieldType::Raster, FieldValue::Binary(b)) => {
            write_varuint(out, b.len() as u64);
            out.extend_from_slice(b);
        }
        (FieldType::Geometry, FieldValue::Geometry(g)) => {
            let blob = encode_geometry_blob(g, &fdef.grid, fdef.geom_has_z, fdef.geom_has_m);
            write_varuint(out, blob.len() as u64);
            out.extend_from_slice(&blob);
        }
        // 类型不匹配的兜底：写与类型相符的空值形式
        (FieldType::String, _) | (FieldType::Xml, _) => {
            write_varuint(out, 0);
        }
        (FieldType::Binary, _) | (FieldType::Raster, _) | (FieldType::Geometry, _) => {
            write_varuint(out, 0);
        }
        (FieldType::Uuid, _) => out.extend_from_slice(&[0u8; 16]),
        (t, _) => {
            // 定长字段写零
            let n = t.fixed_size();
            out.extend_from_slice(&vec![0u8; n]);
        }
    }
}
