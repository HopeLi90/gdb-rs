//! 错误类型定义。

use std::fmt;

/// 库的统一错误类型。
#[derive(Debug)]
pub enum GdbError {
    /// I/O 错误（文件读写）。
    Io(std::io::Error),
    /// UTF-8 / UTF-16 解码错误。
    Utf(std::string::FromUtf8Error),
    /// 目录不是合法的 .gdb（缺少系统目录表）。
    NotAGeodatabase(String),
    /// 指定的对象（要素类/表/要素数据集）不存在。
    NotFound(String),
    /// 字段名或索引无效。
    InvalidField(String),
    /// 几何类型与操作不匹配（例如在表上取几何）。
    GeometryError(String),
    /// 二进制格式解析失败（含偏移越界、未知类型等）。
    Format(String),
    /// 编辑会话相关的错误（未在编辑中、无进行中的操作等）。
    Edit(String),
    /// 其它通用错误。
    Other(String),
}

impl fmt::Display for GdbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GdbError::Io(e) => write!(f, "IO 错误: {e}"),
            GdbError::Utf(e) => write!(f, "文本编码错误: {e}"),
            GdbError::NotAGeodatabase(s) => write!(f, "不是合法的 File Geodatabase: {s}"),
            GdbError::NotFound(s) => write!(f, "未找到对象: {s}"),
            GdbError::InvalidField(s) => write!(f, "非法字段: {s}"),
            GdbError::GeometryError(s) => write!(f, "几何错误: {s}"),
            GdbError::Format(s) => write!(f, "格式解析错误: {s}"),
            GdbError::Edit(s) => write!(f, "编辑会话错误: {s}"),
            GdbError::Other(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for GdbError {}

impl From<std::io::Error> for GdbError {
    fn from(e: std::io::Error) -> Self {
        GdbError::Io(e)
    }
}

impl From<std::string::FromUtf8Error> for GdbError {
    fn from(e: std::string::FromUtf8Error) -> Self {
        GdbError::Utf(e)
    }
}

/// 库的统一结果类型别名。
pub type Result<T> = std::result::Result<T, GdbError>;
