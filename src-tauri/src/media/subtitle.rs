//! 字幕自动下载（subtitlecat）
//!
//! 匹配成功后按番号从 subtitlecat 查询简体中文字幕，落地为 `<stem>.zh.srt`，
//! 供播放器（Jellyfin/Emby 等）按 ISO 639 语言码自动识别加载。
//!
//! best-effort：任何失败（无结果、无简中、网络错误）都只记日志、返回 `Ok(None)`，
//! 不影响刮削主流程；命名遵循 `<视频名>.zh.srt`，自动被 `media::assets` 的字幕
//! 搬移/重命名/删除逻辑接管，无需改动那套代码。

use std::path::{Path, PathBuf};

use scraper::{Html, Selector};

use crate::resource_scrape::fingerprint_client::{self, shared_client};

/// subtitlecat 站点根地址
const BASE_URL: &str = "https://www.subtitlecat.com";

/// 简体中文字幕落地后缀：`<stem>.zh.srt`。
/// Jellyfin/Emby 认 ISO 639 语言码，`zh` 最稳妥；如需保留「简体」信息可改为 `zh-CN`。
const SUBTITLE_LANG_SUFFIX: &str = "zh";

/// subtitlecat 详情页里简中字幕的语言标识（下载链接文件名形如 `<番号>-zh-CN.srt`）
const SUBTITLECAT_ZH_CODE: &str = "zh-cn";

/// 每次最多探查的详情页数量（搜索结果为模糊匹配，取番号命中的前几条即可）
const MAX_DETAIL_PAGES: usize = 5;

/// 按番号下载简体中文字幕并落地到 `<dir>/<stem>.zh.srt`。
///
/// 返回 `Ok(Some(path))` 表示已落地，`Ok(None)` 表示未找到简中字幕（正常情况，非错误）。
/// 目标文件已存在时直接跳过（幂等）。
pub async fn download_subtitle(
    number: &str,
    dir: &Path,
    stem: &str,
) -> Result<Option<PathBuf>, String> {
    let number = number.trim();
    if number.is_empty() {
        return Ok(None);
    }

    let save_path = dir.join(format!("{}.{}.srt", stem, SUBTITLE_LANG_SUFFIX));
    if save_path.exists() {
        log::info!("[subtitle] event=subtitle_exists path={}", save_path.display());
        return Ok(Some(save_path));
    }

    let client = shared_client()?;

    // 1. 搜索页 → 番号严格匹配的详情页链接（搜索为模糊匹配，必须按番号过滤）
    let search_url = format!("{}/index.php?search={}", BASE_URL, encode_query(number));
    let search_html = fingerprint_client::fetch_html(&client, &search_url).await?;
    let detail_links = pick_detail_links(&search_html, number);
    if detail_links.is_empty() {
        log::info!("[subtitle] event=no_match number={}", number);
        return Ok(None);
    }

    // 2. 逐个详情页找简中(zh-CN)下载直链，命中即下载
    for detail_href in detail_links.iter().take(MAX_DETAIL_PAGES) {
        let detail_url = absolutize(detail_href);
        let detail_html = match fingerprint_client::fetch_html(&client, &detail_url).await {
            Ok(html) => html,
            Err(e) => {
                log::warn!("[subtitle] event=detail_fetch_failed url={} error={}", detail_url, e);
                continue;
            }
        };

        let Some(srt_href) = extract_zh_subtitle_href(&detail_html) else {
            continue;
        };

        let srt_url = absolutize(&srt_href);
        let bytes = match fingerprint_client::fetch_bytes(&client, &srt_url).await {
            Ok(bytes) => bytes,
            Err(e) => {
                log::warn!("[subtitle] event=srt_fetch_failed url={} error={}", srt_url, e);
                continue;
            }
        };
        if bytes.is_empty() {
            continue;
        }

        tokio::fs::write(&save_path, &bytes)
            .await
            .map_err(|e| format!("写入字幕文件失败 {}: {}", save_path.display(), e))?;
        log::info!(
            "[subtitle] event=subtitle_saved number={} path={} size={}",
            number, save_path.display(), bytes.len()
        );
        return Ok(Some(save_path));
    }

    log::info!("[subtitle] event=no_zh_subtitle number={}", number);
    Ok(None)
}

/// 从搜索结果页提取「番号严格命中」的详情页链接（相对/绝对 href 原样返回，供后续 absolutize）。
///
/// subtitlecat 搜索为模糊匹配（搜 ABC-123 可能返回 ABC-041 等无关项），故必须按番号过滤，
/// 只保留 href 里包含目标番号的详情页，避免给视频配上错误番号的字幕。
fn pick_detail_links(search_html: &str, number: &str) -> Vec<String> {
    let doc = Html::parse_document(search_html);
    let Ok(selector) = Selector::parse("a[href]") else {
        return Vec::new();
    };
    let target = normalize_id(number);

    let mut links = Vec::new();
    for a in doc.select(&selector) {
        let Some(href) = a.value().attr("href") else {
            continue;
        };
        if !is_detail_link(href) {
            continue;
        }
        // href 里的番号字母数字连续出现，归一化后按包含匹配即可精确命中
        if normalize_id(href).contains(&target) {
            links.push(href.to_string());
        }
    }
    links
}

/// 判断是否为字幕详情页链接：`subs/<id>/<...>.html`（可带前导 `/`）。
fn is_detail_link(href: &str) -> bool {
    let path = href.strip_prefix('/').unwrap_or(href);
    path.starts_with("subs/") && path.to_ascii_lowercase().ends_with(".html")
}

/// 从详情页提取简体中文(zh-CN)字幕的下载直链（形如 `/subs/<id>/<番号>-zh-CN.srt`）。
fn extract_zh_subtitle_href(detail_html: &str) -> Option<String> {
    let doc = Html::parse_document(detail_html);
    let selector = Selector::parse(r#"a[href$=".srt"]"#).ok()?;
    for a in doc.select(&selector) {
        if let Some(href) = a.value().attr("href") {
            let lower = href.to_ascii_lowercase();
            // 文件名形如 `<番号>-zh-CN.srt`，匹配 `-zh-cn.srt` 结尾以区分繁体 zh-TW
            if lower.ends_with(&format!("-{}.srt", SUBTITLECAT_ZH_CODE)) {
                return Some(href.to_string());
            }
        }
    }
    None
}

/// 把站内相对链接补全为绝对 URL；已是 http(s) 的原样返回。
fn absolutize(href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else if let Some(rest) = href.strip_prefix('/') {
        format!("{}/{}", BASE_URL, rest)
    } else {
        format!("{}/{}", BASE_URL, href)
    }
}

/// 归一化番号：仅保留 ASCII 字母数字并大写，抹平大小写与分隔符差异（`SSIS-001` → `SSIS001`）。
fn normalize_id(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// 最小 URL query 编码：番号一般仅含字母数字与 `-`，其余字节转百分号编码。
fn encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_id_strips_separators_and_uppercases() {
        assert_eq!(normalize_id("ssis-001"), "SSIS001");
        assert_eq!(normalize_id("SSIS-001 eng"), "SSIS001ENG");
    }

    #[test]
    fn is_detail_link_matches_subs_html_only() {
        assert!(is_detail_link("subs/252/SSIS-001.html"));
        assert!(is_detail_link("/subs/252/SSIS-001.html"));
        assert!(!is_detail_link("/./index.php"));
        assert!(!is_detail_link("//www.opensubtitles.org"));
        assert!(!is_detail_link("subs/252/SSIS-001-zh-CN.srt"));
    }

    #[test]
    fn absolutize_handles_relative_and_absolute() {
        assert_eq!(absolutize("subs/252/x.html"), "https://www.subtitlecat.com/subs/252/x.html");
        assert_eq!(absolutize("/subs/253/x.srt"), "https://www.subtitlecat.com/subs/253/x.srt");
        assert_eq!(absolutize("https://x.com/a"), "https://x.com/a");
    }

    #[test]
    fn pick_detail_links_filters_by_number_strictly() {
        // 搜 SSIS-001：命中含番号的详情页，忽略无关项与非详情链接
        let html = r#"
            <a href="/./index.php">Home</a>
            <a href="subs/252/SSIS-001.html">SSIS-001</a>
            <a href="subs/254/SSIS-001%20eng.html">SSIS-001 eng</a>
            <a href="subs/999/ABP-041.html">ABP-041</a>
        "#;
        let links = pick_detail_links(html, "SSIS-001");
        assert_eq!(links.len(), 2);
        assert!(links.iter().all(|l| l.contains("SSIS-001")));

        // 模糊结果不含目标番号 → 全部拒绝（对应 subtitlecat 搜 ABP-123 返回 ABP-041 的情况）
        let fuzzy = pick_detail_links(html, "ABP-123");
        assert!(fuzzy.is_empty());
    }

    #[test]
    fn extract_zh_subtitle_href_picks_simplified_only() {
        // 简中(zh-CN)与繁体(zh-TW)并存时只取简中
        let html = r#"
            <a href="/subs/252/SSIS-001-zh-TW.srt" class="green-link">Download</a>
            <a href="/subs/253/SSIS-001-zh-CN.srt" class="green-link">Download</a>
            <a href="/subs/252/SSIS-001-en.srt" class="green-link">Download</a>
        "#;
        assert_eq!(
            extract_zh_subtitle_href(html).as_deref(),
            Some("/subs/253/SSIS-001-zh-CN.srt")
        );

        // 无简中 → None（不回退繁体）
        let no_zh = r#"<a href="/subs/252/SSIS-001-zh-TW.srt">Download</a>"#;
        assert!(extract_zh_subtitle_href(no_zh).is_none());
    }
}
