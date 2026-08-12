//! 数据源注册表与资源网站配置
//!
//! 定义 Source trait、ResourceSite 结构体，
//! 以及数据源注册和默认网站配置函数。

pub mod av123;
pub mod avsox;
pub mod common;
pub mod freejavbt;
pub mod javbus;
pub mod javgg;
pub mod javguru;
pub mod javlibrary;
pub mod javmenu;
pub mod javmost;
pub mod javplace;
pub mod javsb;
pub mod javtiful;
pub mod javxx;
pub mod myjav;
pub mod projectjav;
pub mod sextb;
pub mod threexplanet;

#[cfg(test)]
mod parser_robustness_test;

use serde::{Deserialize, Serialize};
pub use super::types::{ActorAvatar, SearchResult};

/// 数据源对有码/无码作品的支持能力（有码无码分轨：按番号类型路由数据源）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceCapability {
    /// 有码无码都支持（多数综合源，默认）
    General,
    /// 仅有码
    CensoredOnly,
    /// 仅无码（无码专用源，如 avsox）
    UncensoredOnly,
}

impl SourceCapability {
    /// 该能力的源是否应处理给定类型的番号（is_uncensored=true 表示无码作品）。
    pub fn handles(&self, is_uncensored: bool) -> bool {
        match self {
            SourceCapability::General => true,
            SourceCapability::CensoredOnly => !is_uncensored,
            SourceCapability::UncensoredOnly => is_uncensored,
        }
    }
}

/// 数据源 trait
///
/// 每个数据源实现 `parse(html) -> Option<SearchResult>` 和 `build_url(code) -> String`。
/// 搜索时并发请求所有数据源，收集成功结果。
pub trait Source: Send + Sync {
    /// 数据源名称
    fn name(&self) -> &str;
    /// 根据番号构建请求 URL
    fn build_url(&self, code: &str) -> String;
    /// 解析 HTML 提取搜索结果
    fn parse(&self, html: &str, code: &str) -> Option<SearchResult>;
    /// 从搜索结果页提取详情页 URL（需要二次请求的数据源覆盖此方法）
    fn extract_detail_url(&self, _html: &str, _code: &str) -> Option<String> {
        None
    }
    /// 数据源对有码/无码的支持能力（默认综合源，有码无码都查）。
    /// 无码专用源覆盖为 `UncensoredOnly`，纯有码源覆盖为 `CensoredOnly`。
    fn capability(&self) -> SourceCapability {
        SourceCapability::General
    }
}

/// 资源网站定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSite {
    /// 唯一标识，如 "javbus"
    pub id: String,
    /// 显示名称，如 "JavBus"
    pub name: String,
    /// 站点主页 URL，用于设置界面展示（如 "https://www.javbus.com"）
    #[serde(default)]
    pub url: String,
    /// 是否启用
    pub enabled: bool,
    /// 累计平均丰富度得分（0-100），多次刮削结果加权平均
    #[serde(rename = "avgScore", default, skip_serializing_if = "Option::is_none")]
    pub avg_score: Option<u32>,
    /// 累计刮削次数（有效返回结果的次数）
    #[serde(rename = "scrapeCount", default, skip_serializing_if = "Option::is_none")]
    pub scrape_count: Option<u32>,
}

/// 获取所有已注册的数据源
pub fn all_sources() -> Vec<Box<dyn Source>> {
    vec![
        Box::new(javbus::Javbus),
        Box::new(javmenu::Javmenu),
        Box::new(javsb::JavSb),
        Box::new(javxx::JavXX),
        Box::new(javplace::JavPlace),
        Box::new(projectjav::ProjectJav),
        Box::new(threexplanet::ThreeXPlanet),
        Box::new(freejavbt::FreeJavBT),
        Box::new(javlibrary::JavLibrary),
        Box::new(javguru::JavGuru),
        Box::new(javtiful::Javtiful),
        Box::new(av123::Av123),
        Box::new(myjav::MyJav),
        Box::new(javgg::JavGG),
        Box::new(javmost::JavMost),
        Box::new(sextb::SexTB),
        Box::new(avsox::Avsox),
    ]
}

/// 返回默认资源网站配置列表
pub fn default_sites() -> Vec<ResourceSite> {
    // (id, 展示名, 站点主页 URL)。名称沿用"数据源 N"，URL 为各源真实主页（与 build_url 域名一致）。
    const SITES: &[(&str, &str, &str)] = &[
        ("javbus", "数据源 1", "https://www.javbus.com"),
        ("javmenu", "数据源 2", "https://javmenu.com"),
        ("javsb", "数据源 3", "https://jav.sb"),
        ("javxx", "数据源 4", "https://javxx.to"),
        ("javplace", "数据源 5", "https://jav.place"),
        ("projectjav", "数据源 6", "https://projectjav.com"),
        ("3xplanet", "数据源 7", "https://3xplanet.com"),
        ("freejavbt", "数据源 8", "https://freejavbt.com"),
        ("javlibrary", "数据源 9", "https://www.javlibrary.com"),
        ("javguru", "数据源 10", "https://jav.guru"),
        ("javtiful", "数据源 11", "https://javtiful.com"),
        ("123av", "数据源 12", "https://123av.com"),
        ("myjav", "数据源 13", "https://cn.myjav.tv"),
        ("javgg", "数据源 14", "https://javgg.net"),
        ("javmost", "数据源 15", "https://www.javmost.ws"),
        ("sextb", "数据源 16", "https://sextb.net"),
        ("avsox", "数据源 17（无码）", "https://avsox.click"),
    ];

    SITES
        .iter()
        .map(|(id, name, url)| ResourceSite {
            id: id.to_string(),
            name: name.to_string(),
            url: url.to_string(),
            enabled: true,
            avg_score: None,
            scrape_count: None,
        })
        .collect()
}
