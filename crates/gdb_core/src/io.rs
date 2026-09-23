//! 低层二进制读取/写入辅助：小端基本类型、FileGDB 变长整数（varint，
//! 含无符号与 zigzag 有符号）、UTF-16 编解码。

/// 一个基于切片、带位置游标的读取器。
pub struct Reader<'a> {
    pub buf: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    /// 剩余可读字节数。
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn need(&self, n: usize) -> crate::error::Result<()> {
        if self.pos + n > self.buf.len() {
            return Err(crate::error::GdbError::Format(format!(
                "读取越界: 需要 {n} 字节, 剩余 {}",
                self.remaining()
            )));
        }
        Ok(())
    }

    pub fn u8(&mut self) -> crate::error::Result<u8> {
        self.need(1)?;
        let v = self.buf[self.pos];
        self.pos += 1;
        Ok(v)
    }

    pub fn i16(&mut self) -> crate::error::Result<i16> {
        self.need(2)?;
        let v = i16::from_le_bytes([self.buf[self.pos], self.buf[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }

    pub fn u16(&mut self) -> crate::error::Result<u16> {
        self.need(2)?;
        let v = u16::from_le_bytes([self.buf[self.pos], self.buf[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }

    pub fn i32(&mut self) -> crate::error::Result<i32> {
        self.need(4)?;
        let v = i32::from_le_bytes(self.buf[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(v)
    }

    pub fn u32(&mut self) -> crate::error::Result<u32> {
        self.need(4)?;
        let v = u32::from_le_bytes(self.buf[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(v)
    }

    pub fn i64(&mut self) -> crate::error::Result<i64> {
        self.need(8)?;
        let v = i64::from_le_bytes(self.buf[self.pos..self.pos + 8].try_into().unwrap());
        self.pos += 8;
        Ok(v)
    }

    pub fn u64(&mut self) -> crate::error::Result<u64> {
        self.need(8)?;
        let v = u64::from_le_bytes(self.buf[self.pos..self.pos + 8].try_into().unwrap());
        self.pos += 8;
        Ok(v)
    }

    pub fn f32(&mut self) -> crate::error::Result<f32> {
        self.need(4)?;
        let v = f32::from_le_bytes(self.buf[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(v)
    }

    pub fn f64(&mut self) -> crate::error::Result<f64> {
        self.need(8)?;
        let v = f64::from_le_bytes(self.buf[self.pos..self.pos + 8].try_into().unwrap());
        self.pos += 8;
        Ok(v)
    }

    /// 读取 `n` 字节切片（不拷贝）。
    pub fn bytes(&mut self, n: usize) -> crate::error::Result<&'a [u8]> {
        self.need(n)?;
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    /// 无符号变长整数（LEB128，低位组在前）。
    pub fn varuint(&mut self) -> crate::error::Result<u64> {
        let mut result: u64 = 0;
        let mut shift = 0u32;
        loop {
            let b = self.u8()?;
            result |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift >= 64 {
                return Err(crate::error::GdbError::Format("varuint 过长".into()));
            }
        }
        Ok(result)
    }

    /// ESRI 有符号变长整数（与 GDAL `ReadVarIntAndAddNoCheck` 一致，非 zigzag）：
    /// 首字节 bit7=续位、bit6=符号(1=负)、低 6 位数据；续字节 7 位数据，
    /// 位移自 6 起，每次 +7。返回相对 `acc` 的累加结果。
    pub fn varint_esri(&mut self, acc: i64) -> crate::error::Result<i64> {
        let b = self.u8()?;
        let mut val: i64 = (b & 0x3f) as i64;
        let negative = b & 0x40 != 0;
        if b & 0x80 == 0 {
            return Ok(if negative { acc - val } else { acc + val });
        }
        let mut shift = 6u32;
        loop {
            let b2 = self.u8()?;
            val |= ((b2 & 0x7f) as i64) << shift;
            if b2 & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift >= 64 {
                return Err(crate::error::GdbError::Format("varint_esri 过长".into()));
            }
        }
        Ok(if negative { acc - val } else { acc + val })
    }

    /// 读取 UTF-16LE 字符串（长度为"u16 字符数" nchars 个）。
    pub fn utf16(&mut self, nchars: usize) -> crate::error::Result<String> {
        let mut units = Vec::with_capacity(nchars);
        for _ in 0..nchars {
            units.push(self.u16()?);
        }
        Ok(String::from_utf16_lossy(&units))
    }
}

/// 无符号变长整数写入（LEB128）。
pub fn write_varuint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
        if v == 0 {
            break;
        }
    }
}

/// ESRI 有符号变长整数写入（与 `Reader::varint_esri` 对称）：
/// 首字节低 6 位数据、bit6=符号、bit7=续位；续字节 7 位数据（shift 6 起）。
pub fn write_varint_esri(out: &mut Vec<u8>, delta: i64) {
    // 拆分符号与幅度：负数写入 -delta（幅度）。
    let negative = delta < 0;
    let mut mag = if negative { -(delta as i128) as u64 } else { delta as u64 };
    // 首 6 位
    let mut b = (mag & 0x3f) as u8;
    mag >>= 6;
    if negative {
        b |= 0x40;
    }
    if mag != 0 {
        b |= 0x80;
        out.push(b);
        // 续字节：7 位一组
        while mag != 0 {
            let mut b2 = (mag & 0x7f) as u8;
            mag >>= 7;
            if mag != 0 {
                b2 |= 0x80;
            }
            out.push(b2);
        }
    } else {
        out.push(b);
    }
}

/// 计算 ESRI 有符号变长整数的编码字节数（用于几何长度预分配）。
#[allow(dead_code)]
pub fn varint_len(v: i64) -> usize {
    let negative = v < 0;
    let mut mag = if negative { -(v as i128) as u64 } else { v as u64 };
    mag >>= 6; // 首 6 位
    let mut n = 1;
    while mag != 0 {
        mag >>= 7;
        n += 1;
    }
    let _ = negative;
    n
}

/// 计算无符号变长整数的编码字节数。
pub fn varuint_len(mut v: u64) -> usize {
    let mut n = 1;
    while v >> 7 != 0 {
        v >>= 7;
        n += 1;
    }
    n
}

/// 把字符串编码为 UTF-16LE 字节（不含长度前缀）。
pub fn encode_utf16(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 2);
    for u in s.encode_utf16() {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}
