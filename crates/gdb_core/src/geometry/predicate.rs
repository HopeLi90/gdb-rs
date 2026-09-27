//! 二维几何**精确**谓词：点/线/面之间的相交、包含、被包含判定。
//!
//! 这些谓词服务于 `QueryFilter::Spatial`（对应 ArcEngine 的 `ISpatialFilter`），
//! 全部在 XY 平面（Z/M 忽略）上运算，坐标类型为 `f64`。
//!
//! 设计取舍：
//! - **精确判定**（非包络框近似）：点在环内用射线法；线-线用方向判定 + 共线在线段上判定；
//!   面-面用「环边相交 ∨ 一方代表点落在另一方内」。
//! - 洞（内环）按**奇偶规则**（even-odd）处理，因此无需依赖环方向即可正确排除洞内区域。
//! - 端点/共线等退化情形由 [`EPS`] 统一容差兜底；坐标量级很大（投影坐标可达 1e8）时
//!   容差按绝对量级固定，对真实矢量数据足够鲁棒。
//!
//! 复杂度：面-面判定为 O(n·m)（n、m 为顶点数），对外层游标做了包络框粗筛（见
//! [`crate::query_filter::SpatialFilter::matches`]），真实数据下可接受。

use crate::geometry::{Geometry, Polygon};

/// 浮点比较容差（绝对量级）。用于端点、共线、边界点的退化判定。
pub const EPS: f64 = 1e-9;

/// 二维点。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    pub fn new(x: f64, y: f64) -> Self {
        Pt { x, y }
    }

    /// 从浮点元组构造。
    pub fn from_tuple(p: (f64, f64)) -> Self {
        Pt { x: p.0, y: p.1 }
    }

    /// 与另一点的平方距离。
    pub fn dist2(&self, o: &Pt) -> f64 {
        let dx = self.x - o.x;
        let dy = self.y - o.y;
        dx * dx + dy * dy
    }

    /// 与另一点是否重合（容差内）。
    pub fn same(&self, o: &Pt) -> bool {
        self.dist2(o) <= EPS * EPS
    }
}

/// 二维向量叉积 `(b-a) × (c-a)`：>0 逆时针、<0 顺时针、=0 共线。
fn cross(a: Pt, b: Pt, c: Pt) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

/// 点 `p` 是否落在线段 `a-b` 的**包围盒**内（含端点，容差内）。
fn on_segment_bbox(a: Pt, b: Pt, p: Pt) -> bool {
    let minx = a.x.min(b.x) - EPS;
    let maxx = a.x.max(b.x) + EPS;
    let miny = a.y.min(b.y) - EPS;
    let maxy = a.y.max(b.y) + EPS;
    p.x >= minx && p.x <= maxx && p.y >= miny && p.y <= maxy
}

/// 点 `p` 是否落在线段 `a-b` 上（含端点与共线情形）。
pub fn point_on_segment(a: Pt, b: Pt, p: Pt) -> bool {
    let c = cross(a, b, p);
    if c.abs() > EPS * scale_of(a, b) {
        return false;
    }
    on_segment_bbox(a, b, p)
}

/// 尺度因子（用于把叉积的相对容差归一到绝对容差）。
fn scale_of(a: Pt, b: Pt) -> f64 {
    (b.x - a.x).abs().max((b.y - a.y).abs()).max(1.0)
}

/// 两线段是否相交（含共线重叠、端点相接、T 型相交）。
pub fn segments_intersect(p1: Pt, p2: Pt, q1: Pt, q2: Pt) -> bool {
    let d1 = cross(q1, q2, p1);
    let d2 = cross(q1, q2, p2);
    let d3 = cross(p1, p2, q1);
    let d4 = cross(p1, p2, q2);
    let sc = scale_of(p1, p2).max(scale_of(q1, q2));
    let tol = EPS * sc;

    // 一般情形：两两异侧（严格跨越）。
    if ((d1 > tol && d2 < -tol) || (d1 < -tol && d2 > tol))
        && ((d3 > tol && d4 < -tol) || (d3 < -tol && d4 > tol))
    {
        return true;
    }
    // 退化情形：任一端点落在另一线段上（含共线重叠）。
    if d1.abs() <= tol && on_segment_bbox(q1, q2, p1) {
        return true;
    }
    if d2.abs() <= tol && on_segment_bbox(q1, q2, p2) {
        return true;
    }
    if d3.abs() <= tol && on_segment_bbox(p1, p2, q1) {
        return true;
    }
    if d4.abs() <= tol && on_segment_bbox(p1, p2, q2) {
        return true;
    }
    false
}

/// 点是否在**单个环**内或边界上（射线法，奇偶规则；边界返回 true）。
pub fn point_in_ring(ring: &[(f64, f64)], p: Pt) -> bool {
    let n = ring.len();
    if n == 0 {
        return false;
    }
    // 显式边界检测：落在任一边上即视为在内（闭合环）。
    for i in 0..n {
        let a = Pt::from_tuple(ring[i]);
        let b = Pt::from_tuple(ring[(i + 1) % n]);
        if point_on_segment(a, b, p) {
            return true;
        }
    }
    // 射线法：向 +x 方向投射，统计跨越次数。
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = ring[i];
        let (xj, yj) = ring[j];
        let intersect = ((yi > p.y) != (yj > p.y))
            && (p.x < (xj - xi) * (p.y - yi) / (yj - yi) + xi);
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// 点是否在**面**内。
///
/// 采用「外环/洞」两阶段判定（不依赖环方向）：
/// 1. 点落在任一环的**边界**上 → 视为在面内（边界闭合）；
/// 2. 否则按奇偶规则（even-odd）统计点相对各环的内外，落在奇数个环内即为在面内
///    （对单外环 + 若干洞的标准面，等价于「在外环内且不在任何洞内」）。
pub fn point_in_polygon(poly: &Polygon, p: Pt) -> bool {
    // 1) 边界优先：落在任一环边上都算在面内（含洞的边界）。
    for ring in &poly.rings {
        let n = ring.len();
        for i in 0..n {
            let a = Pt::from_tuple(ring[i]);
            let b = Pt::from_tuple(ring[(i + 1) % n]);
            if point_on_segment(a, b, p) {
                return true;
            }
        }
    }
    // 2) 严格内部：奇偶规则。
    let mut inside = false;
    for ring in &poly.rings {
        if point_ring_interior(ring, p) {
            inside = !inside;
        }
    }
    inside
}

/// 点是否在环的**严格内部**（不含边界，射线法）。
fn point_ring_interior(ring: &[(f64, f64)], p: Pt) -> bool {
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = ring[i];
        let (xj, yj) = ring[j];
        let intersect =
            ((yi > p.y) != (yj > p.y)) && (p.x < (xj - xi) * (p.y - yi) / (yj - yi) + xi);
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// 取几何的全部「线串」（点/多点视为长度为 1 的退化线；线取 parts；面取 rings）。
fn line_strings_of(g: &Geometry) -> Vec<Vec<(f64, f64)>> {
    match g {
        Geometry::Point(p) => vec![vec![(p.x, p.y)]],
        Geometry::MultiPoint(m) => m.points.iter().map(|p| vec![*p]).collect(),
        Geometry::Polyline(pl) => pl.parts.clone(),
        Geometry::Polygon(pg) => pg.rings.clone(),
    }
}

/// 几何的**代表点**（用于面-面包含判定的探测点）：面取首环首个顶点，其余取首点。
/// 面有洞时首环顶点必在外环上，因此一定位于面内，适合作代表点。
fn representative_point(g: &Geometry) -> Option<Pt> {
    match g {
        Geometry::Point(p) => Some(Pt::new(p.x, p.y)),
        Geometry::MultiPoint(m) => m.points.first().map(|p| Pt::from_tuple(*p)),
        Geometry::Polyline(pl) => pl.parts.first().and_then(|p| p.first()).map(|p| Pt::from_tuple(*p)),
        Geometry::Polygon(pg) => pg.rings.first().and_then(|r| r.first()).map(|p| Pt::from_tuple(*p)),
    }
}

/// 几何是否落在 `poly` 内（全部顶点都在面内）。用于 Containment 判定。
fn all_points_in_polygon(g: &Geometry, poly: &Polygon) -> bool {
    let pts = g.all_points();
    !pts.is_empty() && pts.iter().all(|p| point_in_polygon(poly, Pt::from_tuple(*p)))
}

/// 几何是否落在另一个几何内（`IsWithin(row ⊆ query)` 的语义）。
///
/// - query 为面：row 的全部顶点都在面内。
/// - query 为线/点：row 的每个顶点都落在线/点上（退化情形，尽力判定）。
fn is_within(row: &Geometry, query: &Geometry) -> bool {
    match query {
        Geometry::Polygon(poly) => all_points_in_polygon(row, poly),
        Geometry::Polyline(pl) => {
            let pts = row.all_points();
            !pts.is_empty()
                && pts.iter().all(|r| {
                    let rp = Pt::from_tuple(*r);
                    pl.parts.iter().any(|part| {
                        part.windows(2).any(|w| {
                            point_on_segment(Pt::from_tuple(w[0]), Pt::from_tuple(w[1]), rp)
                        }) || (part.len() == 1 && Pt::from_tuple(part[0]).same(&rp))
                    })
                })
        }
        Geometry::Point(p) => {
            let qp = Pt::new(p.x, p.y);
            row.all_points().iter().all(|r| qp.same(&Pt::from_tuple(*r)))
        }
        Geometry::MultiPoint(mp) => {
            // query 为多点：row 的每个点都需与某个 query 点重合。
            row.all_points().iter().all(|r| {
                let rp = Pt::from_tuple(*r);
                mp.points.iter().any(|q| Pt::from_tuple(*q).same(&rp))
            })
        }
    }
}

/// 两几何是否**相交**（共享至少一个点）。
pub fn intersects(a: &Geometry, b: &Geometry) -> bool {
    // 1) 任一线串之间的线段相交。
    let la = line_strings_of(a);
    let lb = line_strings_of(b);
    for sa in &la {
        for sb in &lb {
            if line_strings_intersect(sa, sb) {
                return true;
            }
        }
    }
    // 2) 无公共边相交时，可能一方完全落在另一方内部（如小面在大面内）。
    //    用代表点互探。
    if let Geometry::Polygon(p) = b {
        if let Some(rp) = representative_point(a) {
            if point_in_polygon(p, rp) {
                return true;
            }
        }
    }
    if let Geometry::Polygon(p) = a {
        if let Some(rp) = representative_point(b) {
            if point_in_polygon(p, rp) {
                return true;
            }
        }
    }
    false
}

/// 两个线串（顶点序列）是否相交。单点线串退化为点-线/点-点判定。
fn line_strings_intersect(sa: &[(f64, f64)], sb: &[(f64, f64)]) -> bool {
    if sa.is_empty() || sb.is_empty() {
        return false;
    }
    if sa.len() == 1 {
        let p = Pt::from_tuple(sa[0]);
        return point_on_line_string(sb, p);
    }
    if sb.len() == 1 {
        let p = Pt::from_tuple(sb[0]);
        return point_on_line_string(sa, p);
    }
    for wa in sa.windows(2) {
        for wb in sb.windows(2) {
            if segments_intersect(
                Pt::from_tuple(wa[0]),
                Pt::from_tuple(wa[1]),
                Pt::from_tuple(wb[0]),
                Pt::from_tuple(wb[1]),
            ) {
                return true;
            }
        }
    }
    false
}

/// 点是否落在线串（顶点序列）上。
fn point_on_line_string(ls: &[(f64, f64)], p: Pt) -> bool {
    if ls.len() == 1 {
        return Pt::from_tuple(ls[0]).same(&p);
    }
    ls.windows(2)
        .any(|w| point_on_segment(Pt::from_tuple(w[0]), Pt::from_tuple(w[1]), p))
}

/// 几何 `outer` 是否**包含**几何 `inner`（`Contains(outer ⊇ inner)`）。
pub fn contains(outer: &Geometry, inner: &Geometry) -> bool {
    // 复用 within 的反向语义。
    is_within(inner, outer)
}

/// 几何 `row` 是否**位于**几何 `query` 之内（`Within(row ⊆ query)`）。
pub fn within(row: &Geometry, query: &Geometry) -> bool {
    is_within(row, query)
}

/// 按环集合判定点是否在面内（供需要自定义点集时使用）。
pub fn point_in_rings(rings: &[Vec<(f64, f64)>], p: Pt) -> bool {
    let mut inside = false;
    for r in rings {
        if point_in_ring(r, p) {
            inside = !inside;
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{MultiPoint, Point, Polyline};

    fn poly(rings: Vec<Vec<(f64, f64)>>) -> Geometry {
        Geometry::Polygon(Polygon { rings })
    }

    #[test]
    fn point_in_ring_inside_outside_boundary() {
        let ring = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert!(point_in_ring(&ring, Pt::new(5.0, 5.0)), "内部");
        assert!(!point_in_ring(&ring, Pt::new(15.0, 5.0)), "外部");
        assert!(point_in_ring(&ring, Pt::new(0.0, 5.0)), "边界上");
        assert!(point_in_ring(&ring, Pt::new(0.0, 0.0)), "顶点");
    }

    #[test]
    fn point_in_polygon_with_hole() {
        // 外环 0..10，内环（洞）4..6：洞内点应判为「不在面内」。
        let g = poly(vec![
            vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
            vec![(4.0, 4.0), (4.0, 6.0), (6.0, 6.0), (6.0, 4.0)],
        ]);
        let Geometry::Polygon(p) = &g else { unreachable!() };
        assert!(point_in_polygon(p, Pt::new(1.0, 1.0)), "环内洞外");
        assert!(!point_in_polygon(p, Pt::new(5.0, 5.0)), "洞内应排除");
        assert!(point_in_polygon(p, Pt::new(4.0, 5.0)), "洞边界视为在内（边界闭合）");
    }

    #[test]
    fn segments_cross_touch_collinear() {
        let a = Pt::new(0.0, 0.0);
        let b = Pt::new(10.0, 0.0);
        // 严格跨越
        assert!(segments_intersect(a, b, Pt::new(5.0, -5.0), Pt::new(5.0, 5.0)));
        // 端点相接（T 型）
        assert!(segments_intersect(a, b, Pt::new(5.0, 0.0), Pt::new(5.0, 9.0)));
        // 共线重叠
        assert!(segments_intersect(a, b, Pt::new(3.0, 0.0), Pt::new(7.0, 0.0)));
        // 平行不交
        assert!(!segments_intersect(a, b, Pt::new(0.0, 1.0), Pt::new(10.0, 1.0)));
        // 共线但不重叠
        assert!(!segments_intersect(a, b, Pt::new(20.0, 0.0), Pt::new(30.0, 0.0)));
    }

    #[test]
    fn intersects_polygon_with_hole_point_in_hole() {
        let big = poly(vec![
            vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
            vec![(4.0, 4.0), (4.0, 6.0), (6.0, 6.0), (6.0, 4.0)],
        ]);
        // 点落在洞里 → 与面不相交
        let hole_pt = Geometry::Point(Point { x: 5.0, y: 5.0 });
        assert!(!intersects(&hole_pt, &big), "洞内点不应相交");
        // 点落在环内 → 相交
        let inside_pt = Geometry::Point(Point { x: 1.0, y: 1.0 });
        assert!(intersects(&inside_pt, &big));
    }

    #[test]
    fn intersects_line_and_polygon() {
        let sq = poly(vec![vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]]);
        // 穿过
        let line = Geometry::Polyline(Polyline { parts: vec![vec![(-5.0, 5.0), (15.0, 5.0)]] });
        assert!(intersects(&line, &sq));
        // 完全在内
        let inner = Geometry::Polyline(Polyline { parts: vec![vec![(1.0, 1.0), (3.0, 3.0)]] });
        assert!(intersects(&inner, &sq));
        // 完全在外
        let outer = Geometry::Polyline(Polyline { parts: vec![vec![(20.0, 20.0), (30.0, 30.0)]] });
        assert!(!intersects(&outer, &sq));
    }

    #[test]
    fn polygon_polygon_intersects_and_disjoint() {
        let a = poly(vec![vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]]);
        let b = poly(vec![vec![(5.0, 5.0), (15.0, 5.0), (15.0, 15.0), (5.0, 15.0)]]);
        assert!(intersects(&a, &b), "重叠");
        let c = poly(vec![vec![(20.0, 20.0), (30.0, 20.0), (30.0, 30.0), (20.0, 30.0)]]);
        assert!(!intersects(&a, &c), "相离");
        // 完全包含（无边界相交）
        let small = poly(vec![vec![(4.0, 4.0), (6.0, 4.0), (6.0, 6.0), (4.0, 6.0)]]);
        assert!(intersects(&a, &small), "小面在大面内应相交");
    }

    #[test]
    fn contains_and_within_symmetry() {
        let big = poly(vec![vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]]);
        let small = poly(vec![vec![(2.0, 2.0), (4.0, 2.0), (4.0, 4.0), (2.0, 4.0)]]);
        assert!(contains(&big, &small), "大面包含小面");
        assert!(within(&small, &big), "小面位于大面内");
        assert!(!contains(&small, &big), "反向不成立");
        // 部分越界
        let edge = poly(vec![vec![(8.0, 8.0), (12.0, 8.0), (12.0, 12.0), (8.0, 12.0)]]);
        assert!(!within(&edge, &big), "越界面不位于大面内");
    }

    #[test]
    fn multipoint_and_point_relations() {
        let mp = Geometry::MultiPoint(MultiPoint {
            points: vec![(1.0, 1.0), (2.0, 2.0), (3.0, 3.0)],
        });
        let sq = poly(vec![vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]]);
        assert!(within(&mp, &sq));
        assert!(intersects(&mp, &sq));
        let outside_mp = Geometry::MultiPoint(MultiPoint { points: vec![(50.0, 50.0)] });
        assert!(!within(&outside_mp, &sq));
        assert!(!intersects(&outside_mp, &sq));
    }
}
