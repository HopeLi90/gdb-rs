//! 字段值类型 [`FieldValue`]、OLE Automation 日期（FileGDB datetime）与 UUID。
//!
//! 本库不依赖任何外部 crate，故在此自行实现最小化的日期与 UUID 类型。

/// FileGDB 的 datetime 以 OLE Automation Date 存储：自 1899-12-30 起的天数（float64）。
pub fn ole_epoch_days() -> i64 {
    days_from_civil(1899, 12, 30)
}

/// 公历日期 → 自 1970-01-01 起的天数（Howard Hinnant 算法）。
pub fn days_from_civil(y: i32, m: i32, d: i32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64;
    let doy = ((153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5) as i64 + (d - 1) as i64;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era as i64 * 146097 + doe - 719468
}

/// 自 1970-01-01 起的天数 → 公历日期（Howard Hinnant 逆算法）。
pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

/// 日期时间（仅用公历字段表示，足以覆盖 FileGDB datetime 需求）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub micros: u32,
}

impl DateTime {
    /// 由 OLE Automation Date（天数, float64）构造。
    pub fn from_ole(days: f64) -> DateTime {
        let total_micros = (days * 86_400_000_000.0).round() as i64;
        let day_i = total_micros.div_euclid(86_400_000_000);
        let micro_in_day = total_micros.rem_euclid(86_400_000_000) as u64;
        let (y, m, d) = civil_from_days(ole_epoch_days() + day_i);
        let hour = (micro_in_day / 3_600_000_000) as u32;
        let minute = ((micro_in_day % 3_600_000_000) / 60_000_000) as u32;
        let second = ((micro_in_day % 60_000_000) / 1_000_000) as u32;
        let micros = (micro_in_day % 1_000_000) as u32;
        DateTime {
            year: y,
            month: m,
            day: d,
            hour,
            minute,
            second,
            micros,
        }
    }

    /// 转为 OLE Automation Date（天数, float64）。
    pub fn to_ole(&self) -> f64 {
        let day_i = days_from_civil(self.year, self.month as i32, self.day as i32) - ole_epoch_days();
        let micro_in_day = self.hour as i64 * 3_600_000_000
            + self.minute as i64 * 60_000_000
            + self.second as i64 * 1_000_000
            + self.micros as i64;
        (day_i * 86_400_000_000 + micro_in_day) as f64 / 86_400_000_000.0
    }

    /// 格式化（ISO 风格）。
    pub fn to_string_iso(&self) -> String {
        if self.micros == 0 {
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                self.year, self.month, self.day, self.hour, self.minute, self.second
            )
        } else {
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:06}",
                self.year, self.month, self.day, self.hour, self.minute, self.second, self.micros
            )
        }
    }
}

/// UUID（16 字节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Uuid(pub [u8; 16]);

impl Uuid {
    pub fn from_bytes(b: [u8; 16]) -> Uuid {
        Uuid(b)
    }
}

impl std::fmt::Display for Uuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let b = &self.0;
        write!(
            f,
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
        )
    }
}

/// 单个字段的取值。覆盖 FileGDB 的 17 种字段类型（2D 几何为主）。
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    /// int16（字段类型 0）。
    Int16(i16),
    /// int32（字段类型 1）。
    Int32(i32),
    /// int64（字段类型 2 的扩展；v4/Pro3.2 出现）。
    Int64(i64),
    /// float32（字段类型 2）。
    Float(f32),
    /// float64（字段类型 3）。
    Double(f64),
    /// 文本（字段类型 4）。
    Text(String),
    /// 日期时间（字段类型 5）。
    DateTime(DateTime),
    /// 对象 ID（字段类型 6）。
    ObjectId(u64),
    /// 几何（字段类型 7）。
    Geometry(crate::geometry::Geometry),
    /// 二进制大对象（字段类型 8）。
    Binary(Vec<u8>),
    /// UUID（字段类型 10/11）。
    Uuid(Uuid),
    /// XML（字段类型 12）。
    Xml(String),
    /// 空值标记（字段可空时）。
    Null,
}

impl FieldValue {
    /// 便于在 `set_by_name` 处构造文本值。
    pub fn text(s: impl Into<String>) -> Self {
        FieldValue::Text(s.into())
    }
}
