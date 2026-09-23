//! 对目录（GDB_Items / GDB_SystemCatalog）`Definition` XML 的轻量解析。
//!
//! 这里只做需求内的少量提取：判断是否为要素数据集、抽取 SRS 的 WKT。
//! 不引入完整 XML 解析器，使用稳健的子串/标签扫描即可满足分类需求。

/// 判断 Definition XML 是否描述一个要素数据集（`<DEFeatureDataset>`）。
pub fn is_feature_dataset(def: &str) -> bool {
    def.contains("<DEFeatureDataset")
}

/// 从 XML 中提取 `<WKT> ... </WKT>` 文本内容（若有）。
pub fn extract_wkt(def: &str) -> Option<String> {
    let open = "<WKT>";
    let close = "</WKT>";
    let s = def.find(open)? + open.len();
    let e = def[s..].find(close)?;
    Some(def[s..s + e].trim().to_string())
}

/// 提取 `<GeometryType>...</GeometryType>` 的文本（esri 几何类型名，如 esriGeometryPoint）。
pub fn extract_geometry_type_name(def: &str) -> Option<String> {
    let open = "<GeometryType>";
    let close = "</GeometryType>";
    let s = def.find(open)? + open.len();
    let e = def[s..].find(close)?;
    let v = def[s..s + e].trim();
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}
