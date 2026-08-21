//! avgood.com 数据源解析器
//!
//! 搜索型网站：搜索页 `https://avgood.com/c/s/?q={CODE}`，
//! 结果链接到详情页 `/c/{id}.html`（结果链接文本以番号开头，据此匹配）。
//!
//! 详情页结构（JavBus 风格布局）：
//! - 标题：`h1.content-title`（"番号 标题"，去掉番号前缀）
//! - 封面：`a.bigImage` 的 href（相对 `/remote/pic/...`，补全为绝对地址）
//! - 字段：`.content-info` 内 `<p><span class="header">标签:</span> 值</p>`
//!   （发行日期 / 长度 / 导演 / 制作商 / 发行商）
//! - 类别：`span.genre a` 文本
//! - 女优：`.star-name a` 文本
//! - 预览图：`a.sample-box` 的 href
//!
//! 注：演员头像 `<img src="/remote">` 为通用占位符（无真实每人头像），故不抓取头像。

use super::common::{
    dedup_strings, extract_head_meta, select_all_attr, select_all_text, select_attr, select_text,
    strip_prefix_ci,
};
use super::{SearchResult, Source};
use regex::Regex;
use scraper::{Html, Selector};
use std::collections::HashMap;
use std::sync::LazyLock;

const BASE: &str = "https://avgood.com";

static A_HREF_SEL: LazyLock<Selector> = LazyLock::new(|| Selector::parse("a[href]").unwrap());
static INFO_P_SEL: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("div.content-info p").unwrap());
static HEADER_SEL: LazyLock<Selector> = LazyLock::new(|| Selector::parse("span.header").unwrap());
// 详情页链接：/c/{数字}.html（排除搜索 /c/s/、标签 /c/t/）
static DETAIL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:https?://[^/]+)?/c/\d+\.html").unwrap());

pub struct AvGood;

/// 把可能的相对 URL 补全为绝对 URL
fn absolutize(u: &str) -> String {
    if u.is_empty() || u.starts_with("http") {
        u.to_string()
    } else if u.starts_with('/') {
        format!("{}{}", BASE, u)
    } else {
        format!("{}/{}", BASE, u)
    }
}

/// 规范化为纯 ASCII 字母数字大写形式，用于番号匹配（忽略符号/中文/大小写）
fn canon(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_uppercase())
        .collect()
}

/// 从 `.content-info` 的 `<p><span class="header">标签:</span> 值</p>` 提取字段名→值映射。
///
/// 值可能多词（如 "S1 第一风格"），故取整段 `<p>` 文本去掉开头 header 前缀，
/// 而非按空白切首词。无 `span.header` 子标签的 `<p>`（类别标题/类别值列表）自然被跳过。
fn extract_info_fields(doc: &Html) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for p in doc.select(&INFO_P_SEL) {
        let header = match p.select(&HEADER_SEL).next() {
            Some(h) => h,
            None => continue,
        };
        let header_norm = header
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let label = header_norm
            .trim_end_matches(':')
            .trim_end_matches('：')
            .trim()
            .to_string();
        if label.is_empty() {
            continue;
        }
        let full = p
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let value = full
            .strip_prefix(&header_norm)
            .unwrap_or(&full)
            .trim()
            .to_string();
        if !value.is_empty() {
            map.insert(label, value);
        }
    }
    map
}

impl Source for AvGood {
    fn name(&self) -> &str {
        "avgood"
    }

    fn build_url(&self, code: &str) -> String {
        format!("{}/c/s/?q={}", BASE, code)
    }

    /// 从搜索结果页找到与番号匹配的详情页 `/c/{id}.html`
    fn extract_detail_url(&self, html: &str, code: &str) -> Option<String> {
        let doc = Html::parse_document(html);
        let want = canon(code);
        if want.is_empty() {
            return None;
        }
        for a in doc.select(&A_HREF_SEL) {
            let href = a.value().attr("href").unwrap_or("");
            if !DETAIL_RE.is_match(href) {
                continue;
            }
            let text = a.text().collect::<Vec<_>>().join(" ");
            if canon(&text).starts_with(&want) {
                return Some(absolutize(href));
            }
        }
        None
    }

    fn parse(&self, html: &str, code: &str) -> Option<SearchResult> {
        let doc = Html::parse_document(html);
        let code_upper = code.trim().to_uppercase();

        let head = extract_head_meta(&doc);

        // 标题：h1.content-title 优先（干净），回退 head.title（截到首个 "_" 前，去站点后缀）
        let raw_title = select_text(&doc, "h1.content-title")
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| match head.title.find('_') {
                Some(i) => head.title[..i].to_string(),
                None => head.title.clone(),
            });
        let title = strip_prefix_ci(raw_title.trim(), &code_upper)
            .trim_start_matches(|c: char| {
                c == '-' || c == ' ' || c == '　' || c == ':' || c == '：'
            })
            .trim()
            .to_string();

        // 封面：a.bigImage href，回退 head og:image
        let cover_url = select_attr(&doc, "a.bigImage", "href")
            .map(|u| absolutize(&u))
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| head.cover_url.clone());

        // 结构化字段
        let info = extract_info_fields(&doc);
        let premiered = info.get("发行日期").cloned().unwrap_or_default();
        let duration = info.get("长度").cloned().unwrap_or_default();
        let director = info.get("导演").cloned().unwrap_or_default();
        let studio = info.get("制作商").cloned().unwrap_or_default();
        let publisher = info.get("发行商").cloned().unwrap_or_default();

        // 女优
        let actor_names = dedup_strings(select_all_text(&doc, ".star-name a"));
        let actors = actor_names.join(", ");

        // 类别：详情页 HTML 存在未闭合标签，html5ever 会把演员链接（格式化元素 `<a>`）
        // 重构复制到 span.genre 下，导致演员名混入类别；故排除与演员同名的项。
        let tags = dedup_strings(
            select_all_text(&doc, "span.genre a")
                .into_iter()
                .filter(|g| !actor_names.contains(g))
                .collect(),
        )
        .join(", ");

        // 预览图
        let thumbs: Vec<String> = select_all_attr(&doc, "a.sample-box", "href")
            .into_iter()
            .map(|u| absolutize(&u))
            .filter(|u| !u.is_empty())
            .collect();

        if title.is_empty() && cover_url.is_empty() {
            return None;
        }

        let tagline = if premiered.is_empty() {
            String::new()
        } else {
            format!("发行日期 {}", premiered)
        };
        let sort_title = if title.is_empty() {
            code_upper.clone()
        } else {
            format!("{} {}", code_upper, title)
        };

        Some(SearchResult {
            code: code_upper,
            title,
            actors,
            duration,
            studio: studio.clone(),
            source: self.name().to_string(),
            cover_url,
            director,
            tags: tags.clone(),
            premiered,
            thumbs,
            tagline,
            sort_title,
            mpaa: "JP-18+".to_string(),
            custom_rating: "JP-18+".to_string(),
            country_code: "JP".to_string(),
            critic_rating: Some(0),
            maker: studio,
            publisher,
            genres: tags,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DETAIL: &str = r#"
    <html>
      <head><title>SNOS-373 测试标题_磁力链接_AvGood</title></head>
      <body>
        <div class="title-section">
          <h1 class="content-title">SNOS-373 测试标题</h1>
          <a class="bigImage" href="/remote/pic/2026/08/21/snos-373_main.jpg"><img></a>
        </div>
        <div class="col-md-3 content-info">
          <p><span class="header">识别码:</span> <span>SNOS-373</span></p>
          <p><span class="header">发行日期:</span> 2026-08-21</p>
          <p><span class="header">长度:</span> 140分钟</p>
          <p><span class="header">导演:</span> <a href="/c/t/5579/">伊纳巴尔</a></p>
          <p><span class="header">制作商:</span> <a href="/c/t/1498/">S1 第一风格</a></p>
          <p><span class="header">发行商:</span> <a href="/c/t/30/">测试发行商</a></p>
          <p class="header">类别:</p>
          <p>
            <span class="genre"><a href="/c/t/23/">单体作品</a></span>
            <span class="genre"><a href="/c/t/29/">乳交</a></span>
            <!-- 模拟未闭合标签导致演员链接被复制进 span.genre：应被排除 -->
            <span class="genre"><a href="/c/t/134372/" title="早坂奏音">早坂奏音</a></span>
          </p>
          <p class="star-show"><span class="header">演员</span>:</p>
          <ul>
            <div class="star-box">
              <li>
                <a href="/c/t/134372/"><img src="/remote" title="早坂奏音"></a>
                <div class="star-name"><a href="/c/t/134372/" title="早坂奏音">早坂奏音</a></div>
              </li>
            </div>
          </ul>
        </div>
        <div class="samples-section">
          <a class="sample-box" href="/remote/pic/2026/08/21/snos-373_add_0.jpg"><img></a>
          <a class="sample-box" href="/remote/pic/2026/08/21/snos-373_add_1.jpg"><img></a>
        </div>
      </body>
    </html>
    "#;

    #[test]
    fn parse_extracts_detail_fields() {
        let r = AvGood.parse(DETAIL, "SNOS-373").expect("应解析成功");
        assert_eq!(r.code, "SNOS-373");
        assert_eq!(r.title, "测试标题");
        assert_eq!(r.cover_url, "https://avgood.com/remote/pic/2026/08/21/snos-373_main.jpg");
        assert_eq!(r.premiered, "2026-08-21");
        assert_eq!(r.duration, "140分钟");
        assert_eq!(r.director, "伊纳巴尔");
        // 多词值不被截断
        assert_eq!(r.studio, "S1 第一风格");
        assert_eq!(r.publisher, "测试发行商");
        assert_eq!(r.actors, "早坂奏音");
        assert!(r.tags.contains("单体作品") && r.tags.contains("乳交"));
        // 演员名不应混入类别（排除 span.genre 下被复制的演员链接）
        assert!(!r.tags.contains("早坂奏音"));
        assert_eq!(r.thumbs.len(), 2);
        assert_eq!(r.thumbs[0], "https://avgood.com/remote/pic/2026/08/21/snos-373_add_0.jpg");
    }

    #[test]
    fn extract_detail_url_matches_code_link() {
        let search = r#"
          <a href="/c/s/?q=SNOS-373">搜索</a>
          <a href="/c/t/23/">单体作品</a>
          <a href="/c/669912.html" target="_blank"> <em>SNOS</em>-<em>373</em> 测试标题 </a>
        "#;
        let url = AvGood.extract_detail_url(search, "SNOS-373").unwrap();
        assert_eq!(url, "https://avgood.com/c/669912.html");
    }

    #[test]
    fn extract_detail_url_none_when_no_match() {
        let search = r#"<a href="/c/669912.html">ABC-001 其他影片</a>"#;
        assert!(AvGood.extract_detail_url(search, "SNOS-373").is_none());
    }
}
