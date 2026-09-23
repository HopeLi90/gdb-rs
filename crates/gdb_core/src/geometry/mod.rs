//! 几何类型与编解码。
//!
//! FileGDB 几何 blob 结构（**不含**行内 `varuint` 长度前缀——前缀由 table.rs
//! 的字段值编解码层处理），参考 GDAL OpenFileGDB `filegdbtable.cpp`
//! （`ReadGeometry`/`ReadPartDefs`）并经真实数据金标准验证（解码周长与存储的
//! `Shape_Length` 在 6 位小数上精确一致）：
//!
//! ```text
//! [varuint gtype]
//! ```
//! - `gtype` 低 8 位为 SHPT 形状码（见 [`shpt`] 模块），
//!   bit31(0x80000000)=含 Z、bit30(0x40000000)=含 M、bit29(0x20000000)=含曲线段。
//! - **点**：`[varuint x+1][varuint y+1]`（值 0 表示空/NaN；真实网格值 = 存储值-1）；
//!   含 Z/M 时依次附加 `[varuint z+1][varuint m+1]`。无包络框。
//! - **多点**：`[varuint nPoints]`（0=空）→ `[4 varuint 包络]` → XY delta×n
//!   → Z delta×n → M delta×n。
//! - **线/面**：`[varuint nPoints]`（0=空）→ `[varuint nParts]`
//!   →（曲线时 `[varuint nCurves]`）→ `[4 varuint 包络 vxmin,vymin,vdx,vdy]`
//!   → `[nParts-1 个 varuint 各部件点数]`（末部件 = nPoints - 前缀和）
//!   → XY delta×nPoints → Z delta×n → M delta×n。
//! - 包络框为 4 个 `varuint`（无符号，负网格坐标按补码表示）。
//! - 坐标 delta 用 **ESRI varint**（非 zigzag：首字节 bit7=续位、bit6=符号、
//!   低 6 位数据；续字节 7 位数据），自 0 累加；
//!   真实坐标 = `grid / scale + origin`。

use crate::error::{GdbError, Result};
use crate::field::PrecisionGrid;
use crate::io::{Reader, write_varint_esri, write_varuint};

/// SHPT 形状码（`gtype` 低 8 位，与 GDAL `ogrpgeogeometry.h` 一致）。
pub mod shpt {
    pub const NULL: u32 = 0;
    pub const POINT: u32 = 1;
    pub const ARC: u32 = 3; // 折线
    pub const POLYGON: u32 = 5;
    pub const MULTIPOINT: u32 = 8;
    pub const POINTZ: u32 = 9;
    pub const ARCZ: u32 = 10;
    pub const POINTZM: u32 = 11;
    pub const ARCZM: u32 = 13;
    pub const POLYGONZM: u32 = 15;
    pub const MULTIPOINTZM: u32 = 18;
    pub const POLYGONZ: u32 = 19;
    pub const MULTIPOINTZ: u32 = 20;
    pub const POINTM: u32 = 21;
    pub const ARCM: u32 = 23;
    pub const POLYGONM: u32 = 25;
    pub const MULTIPOINTM: u32 = 28;
    pub const MULTIPATCHM: u32 = 31;
    pub const MULTIPATCH: u32 = 32;
    pub const GENERAL_POLYLINE: u32 = 50;
    pub const GENERAL_POLYGON: u32 = 51;
    pub const GENERAL_POINT: u32 = 52;
    pub const GENERAL_MULTIPOINT: u32 = 53;
    pub const GENERAL_MULTIPATCH: u32 = 54;
}

/// gtype 高位标志：含 Z。
pub const EXT_SHAPE_Z_FLAG: u32 = 0x8000_0000;
/// gtype 高位标志：含 M。
pub const EXT_SHAPE_M_FLAG: u32 = 0x4000_0000;
/// gtype 高位标志：含曲线段。
pub const EXT_SHAPE_CURVE_FLAG: u32 = 0x2000_0000;

/// 二维点。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// 多点。
#[derive(Debug, Clone, PartialEq)]
pub struct MultiPoint {
    pub points: Vec<(f64, f64)>,
}

/// 折线（可多部件）。
#[derive(Debug, Clone, PartialEq)]
pub struct Polyline {
    pub parts: Vec<Vec<(f64, f64)>>,
}

/// 面（可多环，外环/内环由环方向区分）。
#[derive(Debug, Clone, PartialEq)]
pub struct Polygon {
    pub rings: Vec<Vec<(f64, f64)>>,
}

/// 几何对象（2D 点/多点/线/面）。
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    Point(Point),
    MultiPoint(MultiPoint),
    Polyline(Polyline),
    Polygon(Polygon),
}

impl Geometry {
    /// 几何包络框（minx, miny, maxx, maxy）。
    pub fn envelope(&self) -> Option<(f64, f64, f64, f64)> {
        let mut pts: Vec<(f64, f64)> = Vec::new();
        match self {
            Geometry::Point(p) => pts.push((p.x, p.y)),
            Geometry::MultiPoint(m) => pts.extend(m.points.iter().copied()),
            Geometry::Polyline(pl) => pl.parts.iter().for_each(|p| pts.extend(p.iter().copied())),
            Geometry::Polygon(pg) => pg.rings.iter().for_each(|r| pts.extend(r.iter().copied())),
        }
        if pts.is_empty() {
            return None;
        }
        let mut minx = f64::INFINITY;
        let mut miny = f64::INFINITY;
        let mut maxx = f64::NEG_INFINITY;
        let mut maxy = f64::NEG_INFINITY;
        for (x, y) in pts {
            minx = minx.min(x);
            miny = miny.min(y);
            maxx = maxx.max(x);
            maxy = maxy.max(y);
        }
        Some((minx, miny, maxx, maxy))
    }

    /// 全部二维坐标点（按部件展开）。
    pub fn all_points(&self) -> Vec<(f64, f64)> {
        let mut out = Vec::new();
        match self {
            Geometry::Point(p) => out.push((p.x, p.y)),
            Geometry::MultiPoint(m) => out.extend(m.points.iter().copied()),
            Geometry::Polyline(pl) => pl.parts.iter().for_each(|p| out.extend(p.iter().copied())),
            Geometry::Polygon(pg) => pg.rings.iter().for_each(|r| out.extend(r.iter().copied())),
        }
        out
    }

    /// SHPT 形状码（不含 Z/M/曲线标志）。
    pub fn shpt_code(&self) -> u32 {
        match self {
            Geometry::Point(_) => shpt::POINT,
            Geometry::MultiPoint(_) => shpt::MULTIPOINT,
            Geometry::Polyline(_) => shpt::ARC,
            Geometry::Polygon(_) => shpt::POLYGON,
        }
    }
}

/// 从几何字段值 blob 解码几何（blob 以 `varuint gtype` 开头，**无长度前缀**）。
pub fn decode_geometry_blob(blob: &[u8], grid: &PrecisionGrid) -> Result<Geometry> {
    let mut r = Reader::new(blob);
    let gtype = r.varuint()? as u32;
    let has_z = gtype & EXT_SHAPE_Z_FLAG != 0;
    let has_m = gtype & EXT_SHAPE_M_FLAG != 0;
    let has_curve = gtype & EXT_SHAPE_CURVE_FLAG != 0;
    let base = gtype & 0xff;

    match base {
        shpt::POINT | shpt::POINTZ | shpt::POINTM | shpt::POINTZM | shpt::GENERAL_POINT => {
            let x = r.varuint()?;
            let y = r.varuint()?;
            if has_z {
                let _ = r.varuint()?;
            }
            if has_m {
                let _ = r.varuint()?;
            }
            if x == 0 || y == 0 {
                return Err(GdbError::GeometryError("空点几何（存储值 0）".into()));
            }
            Ok(Geometry::Point(Point {
                x: grid.grid_to_x(x as i64 - 1),
                y: grid.grid_to_y(y as i64 - 1),
            }))
        }

        shpt::MULTIPOINT
        | shpt::MULTIPOINTZ
        | shpt::MULTIPOINTM
        | shpt::MULTIPOINTZM
        | shpt::GENERAL_MULTIPOINT => {
            let n = r.varuint()? as usize;
            if n == 0 {
                return Ok(Geometry::MultiPoint(MultiPoint { points: vec![] }));
            }
            skip_envelope(&mut r)?;
            let mut pts = Vec::with_capacity(n);
            let mut ax = 0i64;
            let mut ay = 0i64;
            for _ in 0..n {
                ax = r.varint_esri(ax)?;
                ay = r.varint_esri(ay)?;
                pts.push((grid.grid_to_x(ax), grid.grid_to_y(ay)));
            }
            if has_z {
                skip_deltas(&mut r, n)?;
            }
            // 与 GDAL 一致：M 数组缺失时宽松跳过（部分生成器不写 M）。
            if has_m && r.remaining() >= n {
                skip_deltas(&mut r, n)?;
            }
            Ok(Geometry::MultiPoint(MultiPoint { points: pts }))
        }

        shpt::ARC
        | shpt::ARCZ
        | shpt::ARCM
        | shpt::ARCZM
        | shpt::GENERAL_POLYLINE
        | shpt::POLYGON
        | shpt::POLYGONZ
        | shpt::POLYGONM
        | shpt::POLYGONZM
        | shpt::GENERAL_POLYGON => {
            let is_line = matches!(
                base,
                shpt::ARC | shpt::ARCZ | shpt::ARCM | shpt::ARCZM | shpt::GENERAL_POLYLINE
            );
            let n_points = r.varuint()? as usize;
            if n_points == 0 {
                return Ok(if is_line {
                    Geometry::Polyline(Polyline { parts: vec![] })
                } else {
                    Geometry::Polygon(Polygon { rings: vec![] })
                });
            }
            let n_parts = r.varuint()? as usize;
            if has_curve {
                let n_curves = r.varuint()? as usize;
                if n_curves > 0 {
                    return Err(GdbError::GeometryError(format!(
                        "暂不支持曲线段几何（nCurves={n_curves}）"
                    )));
                }
            }
            if n_parts == 0 {
                return Ok(if is_line {
                    Geometry::Polyline(Polyline { parts: vec![] })
                } else {
                    Geometry::Polygon(Polygon { rings: vec![] })
                });
            }
            skip_envelope(&mut r)?;
            // 各部件点数：仅前 nParts-1 个显式存储，末部件 = nPoints - 前缀和。
            let mut counts = Vec::with_capacity(n_parts);
            let mut sum = 0usize;
            for _ in 0..n_parts - 1 {
                let c = r.varuint()? as usize;
                sum = sum.saturating_add(c);
                if sum > n_points {
                    return Err(GdbError::GeometryError("部件点数之和越界".into()));
                }
                counts.push(c);
            }
            counts.push(n_points - sum);

            let mut coords = Vec::with_capacity(n_points);
            let mut ax = 0i64;
            let mut ay = 0i64;
            for _ in 0..n_points {
                ax = r.varint_esri(ax)?;
                ay = r.varint_esri(ay)?;
                coords.push((grid.grid_to_x(ax), grid.grid_to_y(ay)));
            }
            if has_z {
                skip_deltas(&mut r, n_points)?;
            }
            if has_m {
                skip_deltas(&mut r, n_points)?;
            }

            let mut segments = Vec::with_capacity(n_parts);
            let mut start = 0usize;
            for c in counts {
                let end = start + c;
                if end > coords.len() {
                    return Err(GdbError::GeometryError("部件点数越界".into()));
                }
                segments.push(coords[start..end].to_vec());
                start = end;
            }
            if is_line {
                Ok(Geometry::Polyline(Polyline { parts: segments }))
            } else {
                Ok(Geometry::Polygon(Polygon { rings: segments }))
            }
        }

        shpt::MULTIPATCH | shpt::MULTIPATCHM | shpt::GENERAL_MULTIPATCH => Err(
            GdbError::GeometryError("暂不支持多补丁（MultiPatch）几何".into()),
        ),
        other => Err(GdbError::GeometryError(format!(
            "不支持的几何形状码 {other}"
        ))),
    }
}

/// 跳过 4 个 varuint 包络框（vxmin, vymin, vdx, vdy）。
fn skip_envelope(r: &mut Reader<'_>) -> Result<()> {
    for _ in 0..4 {
        r.varuint()?;
    }
    Ok(())
}

/// 跳过 n 个 ESRI varint 增量（Z/M 数组）。
fn skip_deltas(r: &mut Reader<'_>, n: usize) -> Result<()> {
    let mut acc = 0i64;
    for _ in 0..n {
        acc = r.varint_esri(acc)?;
    }
    Ok(())
}

/// 将几何编码为 blob（`varuint gtype` 开头，**无长度前缀**——由调用方负责）。
///
/// `has_z`/`has_m` 决定 gtype 标志位与 Z/M 增量数组；当前几何模型为 2D，
/// Z/M 数组以全 0 增量占位（平坦高度），保证布局合法、可被 ArcGIS 读取。
pub fn encode_geometry_blob(
    geom: &Geometry,
    grid: &PrecisionGrid,
    has_z: bool,
    has_m: bool,
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut gtype = geom.shpt_code();
    if has_z {
        gtype |= EXT_SHAPE_Z_FLAG;
    }
    if has_m {
        gtype |= EXT_SHAPE_M_FLAG;
    }
    write_varuint(&mut out, gtype as u64);

    match geom {
        Geometry::Point(p) => {
            // 点存储 grid+1（0 保留为空值语义）。
            let gx = grid.x_to_grid(p.x) + 1;
            let gy = grid.y_to_grid(p.y) + 1;
            write_varuint(&mut out, gx.max(0) as u64);
            write_varuint(&mut out, gy.max(0) as u64);
            if has_z {
                write_varuint(&mut out, 1);
            }
            if has_m {
                write_varuint(&mut out, 1);
            }
        }
        Geometry::MultiPoint(mp) => {
            let pts = &mp.points;
            write_varuint(&mut out, pts.len() as u64);
            if !pts.is_empty() {
                write_envelope(&mut out, geom, grid);
                let mut ax = 0i64;
                let mut ay = 0i64;
                for (x, y) in pts {
                    let gx = grid.x_to_grid(*x);
                    let gy = grid.y_to_grid(*y);
                    write_varint_esri(&mut out, gx - ax);
                    write_varint_esri(&mut out, gy - ay);
                    ax = gx;
                    ay = gy;
                }
                if has_z {
                    for _ in 0..pts.len() {
                        write_varint_esri(&mut out, 0);
                    }
                }
                if has_m {
                    for _ in 0..pts.len() {
                        write_varint_esri(&mut out, 0);
                    }
                }
            }
        }
        Geometry::Polyline(pl) => encode_parts(&mut out, &pl.parts, geom, grid, has_z, has_m),
        Geometry::Polygon(pg) => encode_parts(&mut out, &pg.rings, geom, grid, has_z, has_m),
    }
    out
}

/// 编码折线/面的部件结构：nPoints → nParts → 包络 → 部件计数 → 增量坐标。
fn encode_parts(
    out: &mut Vec<u8>,
    parts: &[Vec<(f64, f64)>],
    geom: &Geometry,
    grid: &PrecisionGrid,
    has_z: bool,
    has_m: bool,
) {
    let total: usize = parts.iter().map(|p| p.len()).sum();
    write_varuint(out, total as u64); // nPoints
    if total == 0 {
        return;
    }
    write_varuint(out, parts.len() as u64); // nParts
    if parts.is_empty() {
        return;
    }
    write_envelope(out, geom, grid);
    // 前 nParts-1 个部件点数
    for p in &parts[..parts.len() - 1] {
        write_varuint(out, p.len() as u64);
    }
    // XY 增量
    let mut ax = 0i64;
    let mut ay = 0i64;
    for p in parts {
        for (x, y) in p {
            let gx = grid.x_to_grid(*x);
            let gy = grid.y_to_grid(*y);
            write_varint_esri(out, gx - ax);
            write_varint_esri(out, gy - ay);
            ax = gx;
            ay = gy;
        }
    }
    if has_z {
        for _ in 0..total {
            write_varint_esri(out, 0);
        }
    }
    if has_m {
        for _ in 0..total {
            write_varint_esri(out, 0);
        }
    }
}

/// 写入 4 个 varuint 包络框（vxmin, vymin, vdx, vdy；补码无符号）。
fn write_envelope(out: &mut Vec<u8>, geom: &Geometry, grid: &PrecisionGrid) {
    let (minx_g, miny_g, maxx_g, maxy_g) = bbox_grid(geom, grid);
    write_varuint(out, minx_g as u64);
    write_varuint(out, miny_g as u64);
    write_varuint(out, (maxx_g - minx_g) as u64);
    write_varuint(out, (maxy_g - miny_g) as u64);
}

/// 计算几何在精度网格下的包络框整数（网格坐标系）。
fn bbox_grid(geom: &Geometry, grid: &PrecisionGrid) -> (i64, i64, i64, i64) {
    let mut minx = i64::MAX;
    let mut miny = i64::MAX;
    let mut maxx = i64::MIN;
    let mut maxy = i64::MIN;
    for (x, y) in geom.all_points() {
        let gx = grid.x_to_grid(x);
        let gy = grid.y_to_grid(y);
        minx = minx.min(gx);
        miny = miny.min(gy);
        maxx = maxx.max(gx);
        maxy = maxy.max(gy);
    }
    if minx > maxx {
        (0, 0, 0, 0)
    } else {
        (minx, miny, maxx, maxy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> PrecisionGrid {
        PrecisionGrid {
            xorig: 40538389.49,
            yorig: 3044658.394,
            xyscale: 5900.0,
            ..PrecisionGrid::default()
        }
    }

    #[test]
    fn point_roundtrip() {
        let g = grid();
        let p = Geometry::Point(Point { x: 40538500.5, y: 3044700.25 });
        let blob = encode_geometry_blob(&p, &g, false, false);
        let back = decode_geometry_blob(&blob, &g).unwrap();
        match back {
            Geometry::Point(q) => {
                assert!((q.x - 40538500.5).abs() < 1e-4);
                assert!((q.y - 3044700.25).abs() < 1e-4);
            }
            _ => panic!("应为点几何"),
        }
    }

    #[test]
    fn polygon_roundtrip() {
        let g = grid();
        let pg = Geometry::Polygon(Polygon {
            rings: vec![vec![
                (40538490.0, 3044690.0),
                (40538600.0, 3044695.0),
                (40538620.0, 3044780.0),
                (40538500.0, 3044790.0),
                (40538490.0, 3044690.0),
            ]],
        });
        let blob = encode_geometry_blob(&pg, &g, false, false);
        let back = decode_geometry_blob(&blob, &g).unwrap();
        let pts = back.all_points();
        assert_eq!(pts.len(), 5);
        let orig = pg.all_points();
        for (a, b) in orig.iter().zip(pts.iter()) {
            assert!((a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4);
        }
    }

    #[test]
    fn polyline_multipart_roundtrip() {
        let g = grid();
        let pl = Geometry::Polyline(Polyline {
            parts: vec![
                vec![(40538490.0, 3044690.0), (40538600.0, 3044720.0)],
                vec![(40538600.0, 3044720.0), (40538620.0, 3044780.0), (40538500.0, 3044790.0)],
            ],
        });
        let blob = encode_geometry_blob(&pl, &g, false, false);
        let back = decode_geometry_blob(&blob, &g).unwrap();
        match back {
            Geometry::Polyline(p) => {
                assert_eq!(p.parts.len(), 2);
                assert_eq!(p.parts[0].len(), 2);
                assert_eq!(p.parts[1].len(), 3);
            }
            _ => panic!("应为折线几何"),
        }
    }
}
