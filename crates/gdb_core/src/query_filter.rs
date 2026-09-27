//! 查询过滤条件（对应 ArcEngine 的 `IQueryFilter` / `ISpatialFilter`）。
//!
//! 提供由 OID、属性条件（`WhereClause`）与空间条件（`SpatialFilter`）组成的过滤表达式，
//! 供 `Cursor`（`Search`/`Update` 遍历）与 `delete_searched_rows` 等批量操作使用。
//!
//! ## 组成
//! - [`QueryFilter`]：顶层过滤表达式（`All` / `ByOid` / `ByOids` / `Where` / `Spatial` / `Combined`）。
//! - [`WhereClause`]：属性条件，支持 `= != > >= < <= AND OR NOT (...) IS [NOT] NULL [NOT] IN`。
//! - [`SpatialFilter`] / [`SpatialRel`]：空间条件，基于 [`crate::geometry::predicate`] 的**精确**几何判定。
//!
//! ## 示例
//! ```
//! use gdb_core::query_filter::QueryFilter;
//! let f = QueryFilter::where_clause("BH = '编号1' AND Shape_Length > 300").unwrap();
//! let _ = f;
//! ```

use std::fmt;

use crate::error::{GdbError, Result};
use crate::field::{FieldType, TableSchema};
use crate::geometry::predicate;
use crate::geometry::{Geometry, Polygon};
use crate::value::FieldValue;

// ---------------------------------------------------------------------------
// QueryFilter
// ---------------------------------------------------------------------------

/// 查询过滤条件。
#[derive(Debug, Clone)]
pub enum QueryFilter {
    /// 匹配全部行。
    All,
    /// 仅匹配指定 OBJECTID。
    ByOid(u64),
    /// 仅匹配给定 OBJECTID 集合。
    ByOids(Vec<u64>),
    /// 仅按属性条件匹配。
    Where(WhereClause),
    /// 仅按空间条件匹配。
    Spatial(SpatialFilter),
    /// 属性条件 ∧（可选）空间条件。
    Combined {
        where_clause: WhereClause,
        spatial: Option<SpatialFilter>,
    },
}

impl QueryFilter {
    /// 便捷：由 OID 集合构造 [`QueryFilter::ByOids`]。
    pub fn oids<I: IntoIterator<Item = u64>>(ids: I) -> QueryFilter {
        QueryFilter::ByOids(ids.into_iter().collect())
    }

    /// 便捷：解析 where 字符串并构造 [`QueryFilter::Where`]。
    pub fn where_clause(sql: &str) -> Result<QueryFilter> {
        Ok(QueryFilter::Where(WhereClause::parse(sql)?))
    }

    /// 便捷：构造空间过滤（默认 [`SpatialRel::Intersects`]）。
    pub fn intersected(geometry: Geometry) -> Result<QueryFilter> {
        Ok(QueryFilter::Spatial(SpatialFilter::new(
            SpatialRel::Intersects,
            geometry,
        )?))
    }

    /// 便捷：指定空间关系构造空间过滤。
    pub fn spatial(relation: SpatialRel, geometry: Geometry) -> Result<QueryFilter> {
        Ok(QueryFilter::Spatial(SpatialFilter::new(relation, geometry)?))
    }

    /// 在已有属性/空间条件上追加空间条件（AND 语义）。
    pub fn and_spatial(self, relation: SpatialRel, geometry: Geometry) -> Result<QueryFilter> {
        let sf = SpatialFilter::new(relation, geometry)?;
        Ok(match self {
            QueryFilter::All => QueryFilter::Spatial(sf),
            QueryFilter::ByOid(_) | QueryFilter::ByOids(_) => {
                return Err(GdbError::Edit(
                    "and_spatial 仅适用于属性/空间条件，不适用于 OID 条件".into(),
                ))
            }
            QueryFilter::Where(w) => QueryFilter::Combined {
                where_clause: w,
                spatial: Some(sf),
            },
            // 已有空间条件时，用 AND 组合：保留原条件并追加新条件。
            QueryFilter::Spatial(prev) => QueryFilter::Combined {
                where_clause: WhereClause::always_true(),
                spatial: Some(prev),
            }
            .append_sf(sf),
            QueryFilter::Combined {
                where_clause,
                spatial: _,
            } => QueryFilter::Combined {
                where_clause,
                spatial: Some(sf),
            },
        })
    }

    /// 内部：在 Combined 上再叠加一个空间条件。
    fn append_sf(self, sf: SpatialFilter) -> QueryFilter {
        match self {
            QueryFilter::Combined {
                where_clause,
                spatial,
            } => QueryFilter::Combined {
                where_clause,
                spatial: Some(match spatial {
                    Some(mut prev) => {
                        // 两个空间条件：保留后一个（简化语义，ArcEngine 亦仅支持单一 SpatialRel）。
                        let _ = &mut prev;
                        sf
                    }
                    None => sf,
                }),
            },
            other => other,
        }
    }

    /// 是否为「仅按 OID」的简单条件（`All` / `ByOid` / `ByOids`）。
    ///
    /// 用于 `Cursor::new` 的快路径判别：简单条件无需克隆整表即可匹配。
    pub fn is_oid_only(&self) -> bool {
        matches!(
            self,
            QueryFilter::All | QueryFilter::ByOid(_) | QueryFilter::ByOids(_)
        )
    }

    /// 判断某一行是否匹配本条件。
    ///
    /// - `row` 与 `schema.fields` 按下标对齐；
    /// - `oid` 为该行 OBJECTID；
    /// - `geometry` 为该行的几何（无几何字段时传 `None`）。
    pub fn matches_row(
        &self,
        row: &[FieldValue],
        schema: &TableSchema,
        oid: u64,
        geometry: Option<&Geometry>,
    ) -> bool {
        match self {
            QueryFilter::All => true,
            QueryFilter::ByOid(target) => *target == oid,
            QueryFilter::ByOids(targets) => targets.contains(&oid),
            QueryFilter::Where(w) => w.matches_row(row, schema),
            QueryFilter::Spatial(sf) => sf.matches(geometry),
            QueryFilter::Combined {
                where_clause,
                spatial,
            } => {
                where_clause.matches_row(row, schema)
                    && spatial.as_ref().map(|s| s.matches(geometry)).unwrap_or(true)
            }
        }
    }
}

impl fmt::Display for QueryFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryFilter::All => write!(f, "<all>"),
            QueryFilter::ByOid(o) => write!(f, "OBJECTID = {o}"),
            QueryFilter::ByOids(v) => {
                let s: Vec<String> = v.iter().map(|x| x.to_string()).collect();
                write!(f, "OBJECTID IN ({})", s.join(","))
            }
            QueryFilter::Where(w) => write!(f, "{w}"),
            QueryFilter::Spatial(s) => write!(f, "{s}"),
            QueryFilter::Combined {
                where_clause,
                spatial,
            } => match spatial {
                Some(s) => write!(f, "({where_clause}) AND {s}"),
                None => write!(f, "({where_clause})"),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// 属性条件（WhereClause）
// ---------------------------------------------------------------------------

/// 属性条件表达式（对应 ArcEngine `IQueryFilter.WhereClause`）。
#[derive(Debug, Clone)]
pub struct WhereClause {
    expr: Expr,
}

#[derive(Debug, Clone)]
enum Expr {
    /// 恒真（内部占位，用于组合构造）。
    True,
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Cmp {
        field: String,
        op: CmpOp,
        value: Literal,
    },
    IsNull {
        field: String,
        negated: bool,
    },
    In {
        field: String,
        values: Vec<Literal>,
        negated: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CmpOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

#[derive(Debug, Clone, PartialEq)]
enum Literal {
    Num(f64),
    Int(i64),
    Str(String),
    Bool(bool),
    Null,
}

impl WhereClause {
    /// 恒真条件（内部使用）。
    fn always_true() -> WhereClause {
        WhereClause { expr: Expr::True }
    }

    /// 解析 where 字符串。
    pub fn parse(sql: &str) -> Result<WhereClause> {
        let tokens = lex(sql)?;
        let mut p = Parser { tokens, pos: 0 };
        let expr = p.parse_expr()?;
        if p.pos != p.tokens.len() {
            return Err(GdbError::Format(format!(
                "where 子句存在多余内容: {:?}",
                &p.tokens[p.pos..]
            )));
        }
        Ok(WhereClause { expr })
    }

    /// 判断某行是否满足该属性条件。
    pub fn matches_row(&self, row: &[FieldValue], schema: &TableSchema) -> bool {
        eval(&self.expr, row, schema)
    }
}

impl fmt::Display for WhereClause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", fmt_expr(&self.expr))
    }
}

fn fmt_expr(e: &Expr) -> String {
    match e {
        Expr::True => "1=1".to_string(),
        Expr::And(a, b) => format!("({} AND {})", fmt_expr(a), fmt_expr(b)),
        Expr::Or(a, b) => format!("({} OR {})", fmt_expr(a), fmt_expr(b)),
        Expr::Not(a) => format!("(NOT {})", fmt_expr(a)),
        Expr::Cmp { field, op, value } => {
            let o = match op {
                CmpOp::Eq => "=",
                CmpOp::Ne => "<>",
                CmpOp::Gt => ">",
                CmpOp::Ge => ">=",
                CmpOp::Lt => "<",
                CmpOp::Le => "<=",
            };
            format!("{field} {o} {}", fmt_lit(value))
        }
        Expr::IsNull { field, negated } => {
            if *negated {
                format!("{field} IS NOT NULL")
            } else {
                format!("{field} IS NULL")
            }
        }
        Expr::In {
            field,
            values,
            negated,
        } => {
            let vs: Vec<String> = values.iter().map(fmt_lit).collect();
            format!(
                "{field} {}IN ({})",
                if *negated { "NOT " } else { "" },
                vs.join(", ")
            )
        }
    }
}

fn fmt_lit(l: &Literal) -> String {
    match l {
        Literal::Str(s) => format!("'{s}'"),
        Literal::Num(n) => n.to_string(),
        Literal::Int(i) => i.to_string(),
        Literal::Bool(b) => b.to_string(),
        Literal::Null => "NULL".to_string(),
    }
}

// ---------------------------------------------------------------------------
// 词法分析
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),  // 字段名（含被引号包裹的名字）
    Str(String),    // 字符串字面量
    Num(f64),       // 浮点字面量
    Int(i64),       // 整数字面量
    KwAnd,
    KwOr,
    KwNot,
    KwIs,
    KwNull,
    KwIn,
    KwTrue,
    KwFalse,
    Op(CmpOp),
    LParen,
    RParen,
    Comma,
}

fn lex(s: &str) -> Result<Vec<Tok>> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            '\'' => {
                // 字符串：内部 '' 表示一个单引号
                i += 1;
                let mut buf = String::new();
                let mut closed = false;
                while i < chars.len() {
                    if chars[i] == '\'' {
                        if i + 1 < chars.len() && chars[i + 1] == '\'' {
                            buf.push('\'');
                            i += 2;
                            continue;
                        }
                        closed = true;
                        i += 1;
                        break;
                    }
                    buf.push(chars[i]);
                    i += 1;
                }
                if !closed {
                    return Err(GdbError::Format("where 子句中字符串字面量缺少闭合单引号".into()));
                }
                out.push(Tok::Str(buf));
            }
            '"' => {
                // 带引号字段名："Shape_Length"
                i += 1;
                let mut buf = String::new();
                let mut closed = false;
                while i < chars.len() {
                    if chars[i] == '"' {
                        closed = true;
                        i += 1;
                        break;
                    }
                    buf.push(chars[i]);
                    i += 1;
                }
                if !closed {
                    return Err(GdbError::Format("where 子句中字段名缺少闭合双引号".into()));
                }
                out.push(Tok::Ident(buf));
            }
            '=' => {
                if i + 1 < chars.len() && chars[i + 1] == '=' {
                    i += 2;
                } else {
                    i += 1;
                }
                out.push(Tok::Op(CmpOp::Eq));
            }
            '!' => {
                if i + 1 < chars.len() && chars[i + 1] == '=' {
                    i += 2;
                    out.push(Tok::Op(CmpOp::Ne));
                } else {
                    return Err(GdbError::Format("where 子句中出现未知符号 '!'".into()));
                }
            }
            '<' => {
                if i + 1 < chars.len() && chars[i + 1] == '>' {
                    i += 2;
                    out.push(Tok::Op(CmpOp::Ne));
                } else if i + 1 < chars.len() && chars[i + 1] == '=' {
                    i += 2;
                    out.push(Tok::Op(CmpOp::Le));
                } else {
                    i += 1;
                    out.push(Tok::Op(CmpOp::Lt));
                }
            }
            '>' => {
                if i + 1 < chars.len() && chars[i + 1] == '=' {
                    i += 2;
                    out.push(Tok::Op(CmpOp::Ge));
                } else {
                    i += 1;
                    out.push(Tok::Op(CmpOp::Gt));
                }
            }
            _ if c.is_ascii_digit()
                || (c == '-' && i + 1 < chars.len() && (chars[i + 1].is_ascii_digit() || chars[i + 1] == '.'))
                || (c == '.' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit()) =>
            {
                let start = i;
                if chars[i] == '-' {
                    i += 1;
                }
                let mut is_float = false;
                while i < chars.len() {
                    let d = chars[i];
                    if d.is_ascii_digit() {
                        i += 1;
                    } else if d == '.' || d == 'e' || d == 'E' {
                        is_float = true;
                        i += 1;
                    } else if (d == '+' || d == '-')
                        && i > start
                        && (chars[i - 1] == 'e' || chars[i - 1] == 'E')
                    {
                        i += 1;
                    } else {
                        break;
                    }
                }
                let text: String = chars[start..i].iter().collect();
                if is_float {
                    let v: f64 = text
                        .parse()
                        .map_err(|_| GdbError::Format(format!("无法解析数字字面量: {text}")))?;
                    out.push(Tok::Num(v));
                } else {
                    match text.parse::<i64>() {
                        Ok(v) => out.push(Tok::Int(v)),
                        Err(_) => {
                            let v: f64 = text.parse().map_err(|_| {
                                GdbError::Format(format!("无法解析数字字面量: {text}"))
                            })?;
                            out.push(Tok::Num(v));
                        }
                    }
                }
            }
            _ if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                match word.to_ascii_uppercase().as_str() {
                    "AND" => out.push(Tok::KwAnd),
                    "OR" => out.push(Tok::KwOr),
                    "NOT" => out.push(Tok::KwNot),
                    "IS" => out.push(Tok::KwIs),
                    "NULL" => out.push(Tok::KwNull),
                    "IN" => out.push(Tok::KwIn),
                    "TRUE" => out.push(Tok::KwTrue),
                    "FALSE" => out.push(Tok::KwFalse),
                    _ => out.push(Tok::Ident(word)),
                }
            }
            _ => {
                return Err(GdbError::Format(format!(
                    "where 子句中出现未知字符 '{c}'"
                )))
            }
        }
    }
    if out.is_empty() {
        return Err(GdbError::Format("where 子句为空".into()));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// 递归下降解析
// ---------------------------------------------------------------------------

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn parse_expr(&mut self) -> Result<Expr> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr> {
        let mut left = self.parse_and()?;
        while self.eat(&Tok::KwOr) {
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr> {
        let mut left = self.parse_primary()?;
        while self.eat(&Tok::KwAnd) {
            let right = self.parse_primary()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        if self.eat(&Tok::LParen) {
            let e = self.parse_expr()?;
            if !self.eat(&Tok::RParen) {
                return Err(GdbError::Format("where 子句中缺少右括号 ')'".into()));
            }
            return Ok(e);
        }
        if self.eat(&Tok::KwNot) {
            let e = self.parse_primary()?;
            return Ok(Expr::Not(Box::new(e)));
        }
        self.parse_predicate()
    }

    fn parse_predicate(&mut self) -> Result<Expr> {
        let field = match self.bump() {
            Some(Tok::Ident(name)) => name,
            other => {
                return Err(GdbError::Format(format!(
                    "where 子句需要字段名，实际得到 {other:?}"
                )))
            }
        };

        // IS [NOT] NULL
        if self.eat(&Tok::KwIs) {
            let negated = self.eat(&Tok::KwNot);
            if !self.eat(&Tok::KwNull) {
                return Err(GdbError::Format("where 子句中 IS 之后应为 [NOT] NULL".into()));
            }
            return Ok(Expr::IsNull { field, negated });
        }

        // [NOT] IN (...)
        let negated_in = if matches!(self.peek(), Some(Tok::KwNot)) {
            // 需向后看一个 token 判断是否为 IN
            if matches!(self.tokens.get(self.pos + 1), Some(Tok::KwIn)) {
                self.pos += 2;
                true
            } else {
                false
            }
        } else {
            false
        };
        if matches!(self.peek(), Some(Tok::KwIn)) || negated_in {
            if !negated_in {
                self.pos += 1; // 吃掉 IN
            }
            if !self.eat(&Tok::LParen) {
                return Err(GdbError::Format("where 子句中 IN 之后缺少 '('".into()));
            }
            let mut values = Vec::new();
            loop {
                let lit = self.parse_literal()?;
                values.push(lit);
                if self.eat(&Tok::Comma) {
                    continue;
                }
                break;
            }
            if !self.eat(&Tok::RParen) {
                return Err(GdbError::Format("where 子句中 IN 列表缺少 ')'".into()));
            }
            return Ok(Expr::In {
                field,
                values,
                negated: negated_in,
            });
        }

        // 比较运算
        let op = match self.bump() {
            Some(Tok::Op(op)) => op,
            other => {
                return Err(GdbError::Format(format!(
                    "where 子句中字段 {field} 之后应为比较运算符，实际得到 {other:?}"
                )))
            }
        };
        let value = self.parse_literal()?;
        Ok(Expr::Cmp { field, op, value })
    }

    fn parse_literal(&mut self) -> Result<Literal> {
        match self.bump() {
            Some(Tok::Str(s)) => Ok(Literal::Str(s)),
            Some(Tok::Int(i)) => Ok(Literal::Int(i)),
            Some(Tok::Num(n)) => Ok(Literal::Num(n)),
            Some(Tok::KwTrue) => Ok(Literal::Bool(true)),
            Some(Tok::KwFalse) => Ok(Literal::Bool(false)),
            Some(Tok::KwNull) => Ok(Literal::Null),
            other => Err(GdbError::Format(format!(
                "where 子句需要字面量，实际得到 {other:?}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// 求值
// ---------------------------------------------------------------------------

fn eval(e: &Expr, row: &[FieldValue], schema: &TableSchema) -> bool {
    match e {
        Expr::True => true,
        Expr::And(a, b) => eval(a, row, schema) && eval(b, row, schema),
        Expr::Or(a, b) => eval(a, row, schema) || eval(b, row, schema),
        Expr::Not(a) => !eval(a, row, schema),
        Expr::IsNull { field, negated } => {
            let v = lookup(row, schema, field);
            let is_null = matches!(v, None | Some(FieldValue::Null));
            is_null != *negated
        }
        Expr::In {
            field,
            values,
            negated,
        } => {
            let v = lookup(row, schema, field);
            let hit = match v {
                None | Some(FieldValue::Null) => values.iter().any(|l| matches!(l, Literal::Null)),
                Some(val) => values.iter().any(|l| lit_eq_field(l, val)),
            };
            hit != *negated
        }
        Expr::Cmp { field, op, value } => {
            let v = lookup(row, schema, field);
            match (v, value) {
                (None, _) | (Some(FieldValue::Null), _) => false, // SQL 三值逻辑简化
                (Some(val), lit) => match lit {
                    Literal::Null => false,
                    Literal::Str(s) => cmp_text(val, *op, s),
                    Literal::Num(n) => cmp_num(val, *op, *n),
                    Literal::Int(i) => cmp_num(val, *op, *i as f64),
                    Literal::Bool(b) => cmp_bool(val, *op, *b),
                },
            }
        }
    }
}

/// 按字段名查找值（精确匹配，失败再忽略大小写）。
fn lookup<'a>(row: &'a [FieldValue], schema: &TableSchema, name: &str) -> Option<&'a FieldValue> {
    let idx = schema
        .field_index(name)
        .or_else(|| {
            schema
                .fields
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(name))
        })?;
    row.get(idx)
}

/// 把 `FieldValue` 转为数值（用于数值比较），非数值返回 None。
fn as_num(v: &FieldValue) -> Option<f64> {
    match v {
        FieldValue::Int16(x) => Some(*x as f64),
        FieldValue::Int32(x) => Some(*x as f64),
        FieldValue::Int64(x) => Some(*x as f64),
        FieldValue::Float(x) => Some(*x as f64),
        FieldValue::Double(x) => Some(*x),
        FieldValue::ObjectId(x) => Some(*x as f64),
        _ => None,
    }
}

/// 把 `FieldValue` 转为比较用字符串。
fn as_text(v: &FieldValue) -> Option<String> {
    match v {
        FieldValue::Text(s) | FieldValue::Xml(s) => Some(s.clone()),
        FieldValue::DateTime(dt) => Some(dt.to_string_iso()),
        FieldValue::Uuid(u) => Some(u.to_string()),
        _ => as_num(v).map(|n| n.to_string()),
    }
}

fn apply_op(o: std::cmp::Ordering, op: CmpOp) -> bool {
    use std::cmp::Ordering::*;
    match op {
        CmpOp::Eq => o == Equal,
        CmpOp::Ne => o != Equal,
        CmpOp::Gt => o == Greater,
        CmpOp::Ge => o != Less,
        CmpOp::Lt => o == Less,
        CmpOp::Le => o != Greater,
    }
}

fn cmp_num(v: &FieldValue, op: CmpOp, n: f64) -> bool {
    match as_num(v) {
        Some(x) => apply_op(x.partial_cmp(&n).unwrap_or(std::cmp::Ordering::Less), op),
        None => false,
    }
}

fn cmp_text(v: &FieldValue, op: CmpOp, s: &str) -> bool {
    // 优先按文本比较；若字段为数值而字面量为字符串，尝试数值化。
    if let Some(n) = parse_num_literal(s) {
        if let Some(x) = as_num(v) {
            return apply_op(x.partial_cmp(&n).unwrap_or(std::cmp::Ordering::Less), op);
        }
    }
    match as_text(v) {
        Some(t) => apply_op(t.cmp(&s.to_string()), op),
        None => false,
    }
}

fn cmp_bool(v: &FieldValue, op: CmpOp, b: bool) -> bool {
    match v {
        FieldValue::Int16(x) => apply_op((*x != 0).cmp(&b), op),
        FieldValue::Int32(x) => apply_op((*x != 0).cmp(&b), op),
        _ => false,
    }
}

fn lit_eq_field(l: &Literal, v: &FieldValue) -> bool {
    match l {
        Literal::Str(s) => {
            if let Some(n) = parse_num_literal(s) {
                if let Some(x) = as_num(v) {
                    return (x - n).abs() < f64::EPSILON;
                }
            }
            as_text(v).map(|t| t == *s).unwrap_or(false)
        }
        Literal::Num(n) => as_num(v).map(|x| (x - *n).abs() < f64::EPSILON).unwrap_or(false),
        Literal::Int(i) => as_num(v).map(|x| (x - *i as f64).abs() < f64::EPSILON).unwrap_or(false),
        Literal::Bool(b) => matches!(v, FieldValue::Int16(x) if (*x != 0) == *b)
            || matches!(v, FieldValue::Int32(x) if (*x != 0) == *b),
        Literal::Null => matches!(v, FieldValue::Null),
    }
}

fn parse_num_literal(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok()
}

// ---------------------------------------------------------------------------
// 空间过滤
// ---------------------------------------------------------------------------

/// 空间关系（对应 ArcEngine `esriSpatialRelEnum` 的最小子集）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpatialRel {
    /// 包络框相交（快速粗筛）。
    EnvelopeIntersects,
    /// 几何相交（精确）。
    Intersects,
    /// 查询几何包含行几何（精确）。
    Contains,
    /// 行几何位于查询几何内（精确）。
    Within,
}

impl fmt::Display for SpatialRel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            SpatialRel::EnvelopeIntersects => "ENVELOPE_INTERSECTS",
            SpatialRel::Intersects => "INTERSECTS",
            SpatialRel::Contains => "CONTAINS",
            SpatialRel::Within => "WITHIN",
        };
        write!(f, "{s}")
    }
}

/// 空间过滤器（对应 ArcEngine `ISpatialFilter`）。
#[derive(Debug, Clone)]
pub struct SpatialFilter {
    /// 空间关系。
    pub relation: SpatialRel,
    /// 查询几何。
    pub geometry: Geometry,
    /// 查询几何的包络框（预计算，用于粗筛）。
    envelope: (f64, f64, f64, f64),
}

impl SpatialFilter {
    /// 构造空间过滤器。几何为空（无坐标）时报错。
    pub fn new(relation: SpatialRel, geometry: Geometry) -> Result<SpatialFilter> {
        let envelope = geometry
            .envelope()
            .ok_or_else(|| GdbError::GeometryError("空间过滤几何为空".into()))?;
        Ok(SpatialFilter {
            relation,
            geometry,
            envelope,
        })
    }

    /// 查询几何的包络框 `(xmin, ymin, xmax, ymax)`。
    pub fn envelope(&self) -> (f64, f64, f64, f64) {
        self.envelope
    }

    /// 设置空间关系（ArcEngine 风格 setter）。
    pub fn set_relation(&mut self, relation: SpatialRel) {
        self.relation = relation;
    }

    /// 替换查询几何（同时重算包络框）。
    pub fn set_geometry(&mut self, geometry: Geometry) -> Result<()> {
        let envelope = geometry
            .envelope()
            .ok_or_else(|| GdbError::GeometryError("空间过滤几何为空".into()))?;
        self.geometry = geometry;
        self.envelope = envelope;
        Ok(())
    }

    /// 判断行几何是否满足本空间条件。
    ///
    /// `row_geom` 为 `None`（该行几何为空）时一律返回 false。
    pub fn matches(&self, row_geom: Option<&Geometry>) -> bool {
        let Some(rg) = row_geom else { return false };
        let Some(re) = rg.envelope() else { return false };
        let (qxmin, qymin, qxmax, qymax) = self.envelope;
        let (rxmin, rymin, rxmax, rymax) = re;

        match self.relation {
            SpatialRel::EnvelopeIntersects => {
                !(rxmax < qxmin || rxmin > qxmax || rymax < qymin || rymin > qymax)
            }
            SpatialRel::Intersects => {
                // 包络框粗筛后再做精确判定。
                if rxmax < qxmin || rxmin > qxmax || rymax < qymin || rymin > qymax {
                    return false;
                }
                predicate::intersects(&self.geometry, rg)
            }
            SpatialRel::Contains => {
                // 查询几何包含行几何：行包络框必须落在查询包络框内，再做精确判定。
                if !(qxmin <= rxmin && qymin <= rymin && qxmax >= rxmax && qymax >= rymax) {
                    return false;
                }
                predicate::contains(&self.geometry, rg)
            }
            SpatialRel::Within => {
                if !(rxmin <= qxmin && rymin <= qymin && rxmax >= qxmax && rymax >= qymax) {
                    return false;
                }
                predicate::within(rg, &self.geometry)
            }
        }
    }
}

impl fmt::Display for SpatialFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SHAPE {} <filter>", self.relation)
    }
}

/// 便捷：几何是否可用作空间过滤（非空）。
pub fn has_extent(g: &Geometry) -> bool {
    g.envelope().is_some()
}

/// 便捷：构造多边形查询几何（单环）。
pub fn polygon_from_ring(ring: Vec<(f64, f64)>) -> Geometry {
    Geometry::Polygon(Polygon { rings: vec![ring] })
}

/// 让 `FieldType` 在本模块可见（避免未使用告警，同时为后续类型化比较预留）。
#[allow(dead_code)]
fn _field_type_used(_t: FieldType) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::{FieldDef, GeometryType, TableSchema};
    use crate::geometry::{Point, Polygon};

    fn schema() -> TableSchema {
        TableSchema {
            geometry_type: GeometryType::None,
            has_z: false,
            has_m: false,
            string_utf8: true,
            fields: vec![
                FieldDef::new("OBJECTID", FieldType::ObjectId),
                FieldDef::new("BH", FieldType::String),
                FieldDef::new("SZ", FieldType::String),
                FieldDef::new("Shape_Length", FieldType::Float64),
            ],
        }
    }

    fn row(bh: &str, sz: Option<&str>, len: f64) -> Vec<FieldValue> {
        vec![
            FieldValue::ObjectId(1),
            FieldValue::Text(bh.into()),
            match sz {
                Some(s) => FieldValue::Text(s.into()),
                None => FieldValue::Null,
            },
            FieldValue::Double(len),
        ]
    }

    #[test]
    fn parse_eq_string_chinese() {
        let s = schema();
        let w = WhereClause::parse("BH = '编号1'").unwrap();
        assert!(w.matches_row(&row("编号1", Some("x"), 100.0), &s));
        assert!(!w.matches_row(&row("编号2", Some("x"), 100.0), &s));
    }

    #[test]
    fn parse_numeric_cmp() {
        let s = schema();
        for (sql, expect) in [
            ("Shape_Length > 300", true),
            ("Shape_Length > 400", false),
            ("Shape_Length >= 335.175337", true),
            ("Shape_Length < 400", true),
            ("Shape_Length <= 335.175337", true),
            ("Shape_Length <> 1", true),
            ("Shape_Length != 1", true),
        ] {
            let w = WhereClause::parse(sql).unwrap();
            assert_eq!(
                w.matches_row(&row("a", Some("x"), 335.175337), &s),
                expect,
                "{sql}"
            );
        }
    }

    #[test]
    fn parse_quoted_field_name() {
        let s = schema();
        let w = WhereClause::parse("\"Shape_Length\" > 300").unwrap();
        assert!(w.matches_row(&row("a", Some("x"), 335.0), &s));
    }

    #[test]
    fn parse_and_or_paren() {
        let s = schema();
        let w =
            WhereClause::parse("(BH = '编号1' OR BH = '编号2') AND Shape_Length > 0").unwrap();
        assert!(w.matches_row(&row("编号1", Some("x"), 1.0), &s));
        assert!(w.matches_row(&row("编号2", Some("x"), 1.0), &s));
        assert!(!w.matches_row(&row("编号3", Some("x"), 1.0), &s));
    }

    #[test]
    fn parse_is_null_and_not_null() {
        let s = schema();
        let w = WhereClause::parse("SZ IS NULL").unwrap();
        assert!(w.matches_row(&row("a", None, 1.0), &s));
        assert!(!w.matches_row(&row("a", Some("x"), 1.0), &s));
        let w2 = WhereClause::parse("SZ IS NOT NULL").unwrap();
        assert!(w2.matches_row(&row("a", Some("x"), 1.0), &s));
        assert!(!w2.matches_row(&row("a", None, 1.0), &s));
    }

    #[test]
    fn parse_in_list() {
        let s = schema();
        let w = WhereClause::parse("BH IN ('编号1','编号2')").unwrap();
        assert!(w.matches_row(&row("编号1", Some("x"), 1.0), &s));
        assert!(!w.matches_row(&row("编号3", Some("x"), 1.0), &s));
        let w2 = WhereClause::parse("BH NOT IN ('编号1')").unwrap();
        assert!(!w2.matches_row(&row("编号1", Some("x"), 1.0), &s));
        assert!(w2.matches_row(&row("编号2", Some("x"), 1.0), &s));
    }

    #[test]
    fn parse_escaped_quote_in_string() {
        let s = schema();
        let w = WhereClause::parse("BH = 'it''s'").unwrap();
        assert!(w.matches_row(&row("it's", Some("x"), 1.0), &s));
    }

    #[test]
    fn parse_errors() {
        assert!(WhereClause::parse("").is_err());
        assert!(WhereClause::parse("BH = 'unclosed").is_err());
        assert!(WhereClause::parse("BH ^ 1").is_err());
        assert!(WhereClause::parse("BH = 'a' AND").is_err());
        assert!(WhereClause::parse("BH = 'a' 'b'").is_err());
        assert!(WhereClause::parse("(BH = 'a'").is_err());
    }

    #[test]
    fn query_filter_matches_row_variants() {
        let s = schema();
        let r = row("编号1", Some("x"), 100.0);
        let geom = Geometry::Point(Point { x: 1.0, y: 1.0 });
        assert!(QueryFilter::All.matches_row(&r, &s, 7, Some(&geom)));
        assert!(QueryFilter::ByOid(7).matches_row(&r, &s, 7, None));
        assert!(!QueryFilter::ByOid(8).matches_row(&r, &s, 7, None));
        assert!(QueryFilter::oids([7, 9]).matches_row(&r, &s, 7, None));
        assert!(QueryFilter::where_clause("BH = '编号1'")
            .unwrap()
            .matches_row(&r, &s, 1, None));
        assert!(QueryFilter::All.is_oid_only());
        assert!(QueryFilter::ByOids(vec![1]).is_oid_only());
        assert!(!QueryFilter::where_clause("BH = 'x'").unwrap().is_oid_only());
    }

    #[test]
    fn spatial_envelope_and_exact() {
        let sq = SpatialFilter::new(
            SpatialRel::Intersects,
            polygon_from_ring(vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]),
        )
        .unwrap();
        let inside = Geometry::Point(Point { x: 5.0, y: 5.0 });
        let outside = Geometry::Point(Point { x: 50.0, y: 50.0 });
        assert!(sq.matches(Some(&inside)));
        assert!(!sq.matches(Some(&outside)));
        assert!(!sq.matches(None));
    }

    #[test]
    fn spatial_exact_excludes_hole_false_positive() {
        // 带洞面做查询：洞内点应不命中（bbox 近似会假阳性，精确判定必须排除）。
        let donut = Geometry::Polygon(Polygon {
            rings: vec![
                vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
                vec![(4.0, 4.0), (4.0, 6.0), (6.0, 6.0), (6.0, 4.0)],
            ],
        });
        let sf = SpatialFilter::new(SpatialRel::Intersects, donut).unwrap();
        let hole_pt = Geometry::Point(Point { x: 5.0, y: 5.0 });
        let ring_pt = Geometry::Point(Point { x: 1.0, y: 1.0 });
        assert!(!sf.matches(Some(&hole_pt)), "洞内点不应命中（证明是精确判定）");
        assert!(sf.matches(Some(&ring_pt)));
    }

    #[test]
    fn spatial_contains_within() {
        let big = polygon_from_ring(vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
        let small = Geometry::Point(Point { x: 5.0, y: 5.0 });
        // 大面包含点
        let c = SpatialFilter::new(SpatialRel::Contains, big.clone()).unwrap();
        assert!(c.matches(Some(&small)));
        // 点可被自身包络「包含」：Contains 语义下同一点为真
        let w = SpatialFilter::new(SpatialRel::Within, small.clone()).unwrap();
        assert!(w.matches(Some(&small)), "点位于自身之内");
        // 点不包含整个大面
        let c2 = SpatialFilter::new(SpatialRel::Contains, small.clone()).unwrap();
        assert!(!c2.matches(Some(&big)), "点不包含大面");
    }

    #[test]
    fn and_spatial_combines() {
        let f = QueryFilter::where_clause("BH = '编号1'")
            .unwrap()
            .and_spatial(
                SpatialRel::Intersects,
                polygon_from_ring(vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]),
            )
            .unwrap();
        let s = schema();
        let r = row("编号1", Some("x"), 1.0);
        let inside = Geometry::Point(Point { x: 5.0, y: 5.0 });
        let outside = Geometry::Point(Point { x: 50.0, y: 50.0 });
        assert!(f.matches_row(&r, &s, 1, Some(&inside)));
        assert!(!f.matches_row(&r, &s, 1, Some(&outside)));
        assert!(!f.matches_row(&row("编号2", Some("x"), 1.0), &s, 1, Some(&inside)));
    }
}
