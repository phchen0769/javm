//! avsox 数据源（无码专用源）
//!
//! avsox.click 已改为纯前端渲染：页面 HTML 只是空壳（1.5KB，`#javu-site-index` 占位），影片数据
//! 全部来自 JSON 接口 `POST {BASE}/javu/data/api/<method>`（body 为参数数组；无需 CSRF，
//! 但需浏览器 UA，简短 UA 会被 403）：
//! - `search`：`[{"search": 番号, "lang": "cn"}, 60, 1]` → 影片列表（`movieId` / `movieFanHao`）
//! - `getMovie`：`[movieId, "cn"]` → 详情（标题 / 封面 / 日期 / 时长 / 厂牌 / 系列 / 类别 / 女优 / 样图）
//!
//! 故该源走 [`Source::fetch_via_api`]。站点收录的番号写法与本地归一略有出入（`SMBD-050`
//! 收录为 `SMBD-50`，FC2 一律 `FC2-PPV-xxx`），搜索时按候选写法逐个尝试并宽松比对。
//!
//! 下方的 HTML 解析（`extract_detail_url` / `parse`）是 moo 家族服务端渲染页面的旧逻辑
//! （搜索页 `a.movie-box` → 详情页 `.info` / `span.genre` / `a.avatar-box`），
//! 仅在接口不可用而某镜像仍返回完整 HTML 时作兜底。

use scraper::{Html, Selector};
use serde_json::{json, Value};
use super::common::{select_all_attr, select_all_text, select_attr, select_text};
use super::{ActorAvatar, ApiFetchFuture, SearchResult, Source, SourceCapability};
use crate::utils::designation_recognizer::{is_d2pass_designation, same_designation};

/// avsox 主域名（该站点有多个镜像，域名随版本更新）
const BASE: &str = "https://avsox.click";
/// JSON 数据接口前缀
const API_BASE: &str = "https://avsox.click/javu/data/api";
/// 接口语言（标题 / 类别 / 女优名按此本地化，缺失时接口自动回退日文）
const API_LANG: &str = "cn";
/// 搜索单页条数
const SEARCH_PAGE_SIZE: u32 = 60;

pub struct Avsox;

impl Source for Avsox {
    fn name(&self) -> &str { "avsox" }

    fn capability(&self) -> SourceCapability {
        SourceCapability::UncensoredOnly
    }

    fn build_url(&self, code: &str) -> String {
        format!("{}/{}/search/{}", BASE, API_LANG, code)
    }

    fn fetch_via_api(&self, code: &str) -> Option<ApiFetchFuture> {
        let code = code.to_string();
        Some(Box::pin(async move { fetch_movie_by_api(&code).await }))
    }

    /// 从搜索结果列表页提取详情页 URL（优先番号匹配，回退第一个）
    fn extract_detail_url(&self, html: &str, code: &str) -> Option<String> {
        let doc = Html::parse_document(html);
        let code_norm = normalize_code(code);
        let code_upper = code.trim().to_uppercase();
        let sel = Selector::parse("a.movie-box").ok()?;

        // 先按原样番号（含分隔符）精确匹配：D2Pass 系纯数字番号的分隔符是厂牌区分符，
        // 加勒比 110615-001 与一本道 110615_001 同时在列时不能混淆
        if !code_upper.is_empty() {
            for el in doc.select(&sel) {
                let text: String = el.text().collect::<Vec<_>>().join(" ");
                if text.to_uppercase().contains(&code_upper) {
                    if let Some(href) = el.value().attr("href").filter(|h| !h.is_empty()) {
                        return Some(absolute(href));
                    }
                }
            }
        }
        // D2Pass 系番号精确未命中即视为无此片：模糊/首项回退会拿到别家的同号影片
        if is_d2pass_designation(code) {
            return None;
        }

        for el in doc.select(&sel) {
            let text: String = el.text().collect::<Vec<_>>().join(" ");
            if normalize_code(&text).contains(&code_norm) {
                if let Some(href) = el.value().attr("href") {
                    if !href.is_empty() {
                        return Some(absolute(href));
                    }
                }
            }
        }

        // 回退：取第一个结果
        let first = doc.select(&sel).next()?;
        first
            .value()
            .attr("href")
            .filter(|h| !h.is_empty())
            .map(absolute)
    }

    fn parse(&self, html: &str, code: &str) -> Option<SearchResult> {
        let doc = Html::parse_document(html);

        // 封面：bigImage href 通常是大图，img src 通常是缩略图
        let cover_url = select_attr(&doc, "a.bigImage", "href")
            .or_else(|| select_attr(&doc, ".bigImage img", "src"))
            .map(|u| absolute(&u))
            .unwrap_or_default();
        let poster_url = select_attr(&doc, ".bigImage img", "src")
            .map(|u| absolute(&u))
            .unwrap_or_default();

        // 标题：h3，去掉番号部分
        let raw_title = select_text(&doc, "h3").unwrap_or_default();
        let title = if raw_title.is_empty() {
            String::new()
        } else {
            raw_title.replace(code, "").trim().to_string()
        };
        let sort_title = if raw_title.is_empty() {
            code.to_string()
        } else {
            format!("{} {}", code, raw_title)
        };

        let info_text = select_text(&doc, ".info").unwrap_or_default();
        let premiered = extract_field(&info_text, &["發行日期:", "发行日期:"]).unwrap_or_default();
        let duration_raw = extract_field(&info_text, &["長度:", "长度:"]).unwrap_or_default();
        let duration = if duration_raw.is_empty() {
            String::new()
        } else {
            duration_raw.replace("分鐘", "分钟")
        };
        let label = extract_field(&info_text, &["系列:"]).unwrap_or_default();
        let studio = extract_field(
            &info_text,
            &["製作商:", "制作商:", "發行商:", "发行商:"],
        )
        .unwrap_or_default();

        // 类别（只取 href 含 /genre/ 的链接）
        let tags = select_genre(&doc).join(", ");

        // 女优：avsox 详情页用 a.avatar-box span，回退 .star-name a
        let mut actor_list = select_all_text(&doc, "a.avatar-box span");
        if actor_list.is_empty() {
            actor_list = select_all_text(&doc, ".star-name a");
        }
        let actors = actor_list.join(", ");

        // 预览截图
        let thumbs = select_all_attr(&doc, "#sample-waterfall a.sample-box", "href")
            .into_iter()
            .map(|u| absolute(&u))
            .collect();

        if title.is_empty() && cover_url.is_empty() {
            return None;
        }

        Some(SearchResult {
            code: code.to_string(),
            title,
            poster_url,
            actors,
            duration,
            studio: studio.clone(),
            source: self.name().to_string(),
            cover_url,
            tags: tags.clone(),
            premiered,
            rating: None,
            thumbs,
            sort_title,
            mpaa: "JP-18+ 无码".to_string(),
            custom_rating: "JP-18+".to_string(),
            country_code: "JP".to_string(),
            critic_rating: Some(0),
            maker: studio,
            label,
            genres: tags,
            is_uncensored: true,
            ..Default::default()
        })
    }
}

// ============ JSON 接口 ============

/// 走 JSON 接口：按候选写法搜索 → 命中即取详情。`Ok(None)` 表示站点未收录。
async fn fetch_movie_by_api(code: &str) -> Result<Option<SearchResult>, String> {
    let client = crate::resource_scrape::fingerprint_client::shared_client()?;
    for query in search_queries(code) {
        let list = api_call(
            &client,
            "search",
            json!([{ "search": query, "lang": API_LANG }, SEARCH_PAGE_SIZE, 1]),
        )
        .await?;
        let movie_id = list
            .as_array()
            .into_iter()
            .flatten()
            .find(|m| m["movieFanHao"].as_str().is_some_and(|f| same_code_loose(f, code)))
            .and_then(|m| m["movieId"].as_str())
            .map(str::to_string);
        let Some(movie_id) = movie_id else {
            continue;
        };
        let detail = api_call(&client, "getMovie", json!([movie_id, API_LANG])).await?;
        return Ok(movie_json_to_result(&detail, code));
    }
    Ok(None)
}

/// 调用一次数据接口，返回 `data` 字段。接口约定：`{"code": 200, "data": ...}`，其余 code 视为错误。
async fn api_call(client: &wreq::Client, method: &str, args: Value) -> Result<Value, String> {
    let url = format!("{}/{}", API_BASE, method);
    let resp = client
        .post(&url)
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Referer", format!("{}/{}", BASE, API_LANG))
        .json(&args)
        .send()
        .await
        .map_err(|e| format!("avsox 接口请求失败 ({}): {}", method, e))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("avsox 接口响应非 JSON ({}): {}", method, e))?;
    match body["code"].as_i64() {
        Some(200) => Ok(body["data"].clone()),
        other => Err(format!(
            "avsox 接口返回错误 ({}): code={:?} message={}",
            method,
            other,
            body["message"].as_str().unwrap_or("")
        )),
    }
}

/// 搜索用的番号写法候选：先按归一番号，再按站点收录习惯改写——序号去前导零（`SMBD-050` → `SMBD-50`）、
/// FC2 补 PPV（`FC2-821825` → `FC2-PPV-821825`）。D2Pass 纯数字番号只用原样（分隔符 / 前导零都有含义）。
fn search_queries(code: &str) -> Vec<String> {
    let code = code.trim().to_uppercase();
    let mut queries = vec![code.clone()];
    if is_d2pass_designation(&code) {
        return queries;
    }
    if let Some((prefix, number)) = code.rsplit_once('-') {
        let trimmed = number.trim_start_matches('0');
        if !trimmed.is_empty() && trimmed != number {
            queries.push(format!("{}-{}", prefix, trimmed));
        }
        if prefix == "FC2" {
            queries.push(format!("FC2-PPV-{}", number));
        }
    }
    queries
}

/// 站点收录的番号是否就是要找的番号：先严格比对（D2Pass 分隔符敏感）；字母前缀番号再宽松比对
/// ——忽略 FC2 的 PPV 段与序号前导零（站点把 SMBD-050 收录为 SMBD-50）。
fn same_code_loose(candidate: &str, code: &str) -> bool {
    if same_designation(candidate, code) {
        return true;
    }
    if is_d2pass_designation(candidate) || is_d2pass_designation(code) {
        return false;
    }
    match (split_code(candidate), split_code(code)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// 拆成（前缀, 去前导零的序号），均大写，去掉 PPV 段；无分隔符或序号非纯数字则不拆。
fn split_code(s: &str) -> Option<(String, String)> {
    let upper = s.trim().to_uppercase();
    let parts: Vec<&str> = upper
        .split(['-', '_', ' '])
        .filter(|p| !p.is_empty() && *p != "PPV")
        .collect();
    let (number, prefix) = parts.split_last()?;
    if prefix.is_empty() {
        return None;
    }
    let number = number.trim_start_matches('0');
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((prefix.join("-"), number.to_string()))
}

/// 取 JSON 字符串字段（去空白，空串视为无）
fn text_of(v: &Value, key: &str) -> Option<String> {
    v[key]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 取本地化名称：`{key}` 已按接口语言解析，缺失时回退日文 / 英文
fn localized_name(v: &Value, key: &str) -> String {
    text_of(v, key)
        .or_else(|| text_of(v, &format!("{}_ja", key)))
        .or_else(|| text_of(v, &format!("{}_en", key)))
        .unwrap_or_default()
}

/// 把 `getMovie` 详情转成搜索结果；无标题且无封面视为无效。
fn movie_json_to_result(m: &Value, code: &str) -> Option<SearchResult> {
    let title = localized_name(m, "title");
    let cover_url = text_of(m, "posterLarge")
        .or_else(|| text_of(m, "posterSmall"))
        .unwrap_or_default();
    if title.is_empty() && cover_url.is_empty() {
        return None;
    }
    let poster_url = text_of(m, "posterSmall").unwrap_or_default();
    let premiered = text_of(m, "releaseDate").unwrap_or_default();
    let duration = m["length"]
        .as_i64()
        .filter(|n| *n > 0)
        .map(|n| format!("{}分钟", n))
        .unwrap_or_default();
    let studio = localized_name(&m["studio"], "studioName");
    let label = localized_name(&m["series"], "seriesName");
    let tags = m["genre"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|g| localized_name(g, "genreName"))
        .filter(|g| !g.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    // star_code 留空：那是 javbus 系演员页的路径码，avsox 的 starId 不通用
    let actor_avatars: Vec<ActorAvatar> = m["star"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let name = localized_name(s, "starName");
            if name.is_empty() {
                return None;
            }
            Some(ActorAvatar {
                name,
                avatar_url: text_of(s, "avatarUrl").unwrap_or_default(),
                star_code: String::new(),
            })
        })
        .collect();
    let actors = actor_avatars
        .iter()
        .map(|a| a.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let thumbs: Vec<String> = m["sampleLarge"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .map(rewrite_sample_url)
        .collect();
    let plot = localized_name(m, "description");
    let page_url = match text_of(m, "movieId") {
        Some(id) => format!("{}/{}/movie/{}", BASE, API_LANG, id),
        None => format!("{}/{}/search/{}", BASE, API_LANG, code),
    };

    Some(SearchResult {
        code: code.to_string(),
        sort_title: format!("{} {}", code, title),
        title,
        actors,
        actor_avatars,
        duration,
        studio: studio.clone(),
        source: "avsox".to_string(),
        page_url,
        cover_url,
        poster_url,
        tags: tags.clone(),
        premiered,
        rating: None,
        thumbs,
        plot,
        mpaa: "JP-18+ 无码".to_string(),
        custom_rating: "JP-18+".to_string(),
        country_code: "JP".to_string(),
        critic_rating: Some(0),
        maker: studio,
        label,
        genres: tags,
        is_uncensored: true,
        ..Default::default()
    })
}

// ============ 辅助函数 ============

/// AVE 系样图地址失效镜像前缀：接口给的 `file.netcdn.space/ave/vodimages/screenshot/…/001.jpg`
/// 一律 403/404（站点自己的详情页也显示不出来）
const DEAD_AVE_SAMPLE_PREFIX: &str = "https://file.netcdn.space/ave/vodimages/screenshot/";
/// AVE 官网图床同路径可直接下载（无需 Referer），但只提供 webp
const AVE_SAMPLE_PREFIX: &str = "https://imgs02.aventertainments.com/archive/vodimages/screenshot/";

/// 改写接口返回的样图地址：AVE 系（S Model / Catwalk Poison / Laforet / Kirari 等）换到官网图床，
/// 其余（HEYZO 等）原样返回。
fn rewrite_sample_url(url: &str) -> String {
    match url.strip_prefix(DEAD_AVE_SAMPLE_PREFIX) {
        Some(rest) => {
            let rest = rest
                .strip_suffix(".jpg")
                .or_else(|| rest.strip_suffix(".jpeg"))
                .map(|stem| format!("{}.webp", stem))
                .unwrap_or_else(|| rest.to_string());
            format!("{}{}", AVE_SAMPLE_PREFIX, rest)
        }
        None => url.to_string(),
    }
}

/// 相对路径补全为绝对 URL
fn absolute(url: &str) -> String {
    if url.starts_with("http") {
        url.to_string()
    } else {
        format!("{}{}", BASE, url)
    }
}

/// 番号归一：去除非字母数字并大写，用于宽松匹配
fn normalize_code(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_uppercase())
        .collect()
}

/// 选取 span.genre 下 href 含 /genre/ 的类别文本
fn select_genre(doc: &Html) -> Vec<String> {
    let sel = match Selector::parse("span.genre a") {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    doc.select(&sel)
        .filter_map(|el| {
            let href = el.value().attr("href").unwrap_or("");
            if !href.contains("/genre/") {
                return None;
            }
            let text: String = el.text().collect::<Vec<_>>().join(" ");
            let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if cleaned.is_empty() { None } else { Some(cleaned) }
        })
        .collect()
}

/// 从信息文本中提取指定字段的值
fn extract_field(text: &str, labels: &[&str]) -> Option<String> {
    for label in labels {
        if let Some(pos) = text.find(label) {
            let after = &text[pos + label.len()..];
            let value = after.trim().split_whitespace().next()?;
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_is_uncensored_only() {
        assert_eq!(Avsox.capability(), SourceCapability::UncensoredOnly);
        assert!(Avsox.capability().handles(true)); // 无码作品：查
        assert!(!Avsox.capability().handles(false)); // 有码作品：跳过
        // 走 JSON 接口，不走 HTML 管线
        assert!(Avsox.fetch_via_api("n0698").is_some());
    }

    #[test]
    fn search_queries_follow_site_naming() {
        // 站点把 SMBD-050 收录为 SMBD-50，FC2 一律带 PPV
        assert_eq!(search_queries("SMBD-050"), vec!["SMBD-050", "SMBD-50"]);
        assert_eq!(
            search_queries("FC2-821825"),
            vec!["FC2-821825", "FC2-PPV-821825"]
        );
        // 无前导零 / 无分隔符：只有原样
        assert_eq!(search_queries("HEYZO-1234"), vec!["HEYZO-1234"]);
        assert_eq!(search_queries("n0698"), vec!["N0698"]);
        // D2Pass 纯数字番号：前导零与分隔符都有含义，不改写
        assert_eq!(search_queries("110615-001"), vec!["110615-001"]);
    }

    #[test]
    fn same_code_loose_ignores_leading_zeros_and_ppv() {
        assert!(same_code_loose("SMBD-50", "SMBD-050"));
        assert!(same_code_loose("FC2-PPV-821825", "FC2-821825"));
        assert!(same_code_loose("n0698", "N0698"));
        assert!(!same_code_loose("SMBD-5", "SMBD-050"));
        assert!(!same_code_loose("SMBD-51", "SMBD-050"));
        assert!(!same_code_loose("SMD-50", "SMBD-050"));
        // D2Pass：分隔符敏感，不做宽松比对
        assert!(same_code_loose("110615-001", "110615-001"));
        assert!(!same_code_loose("110615_001", "110615-001"));
    }

    #[test]
    fn movie_json_maps_detail_fields() {
        // 取自接口真实返回（截取）
        let m = json!({
            "movieId": "jnoqmdn",
            "movieFanHao": "SMBD-50",
            "title": "S Model 50 : 大海まりん (ブルーレイディスク版) ",
            "releaseDate": "2012-03-05",
            "length": 110,
            "posterSmall": "https://file.netcdn.space/storage/ave/archive/jacket_images/dvd1smbd-50.jpg",
            "posterLarge": "https://file.netcdn.space/storage/ave/archive/bigcover/dvd1smbd-50.jpg",
            "sampleLarge": ["https://file.netcdn.space/ave/vodimages/screenshot/large/SMBD-50/001.jpg"],
            "description_cn": null,
            "description_ja": null,
            "series": { "seriesName": "S Model - Blu-ray" },
            "studio": { "studioName": "スーパーモデルメディア", "studioName_en": "Super Model Media" },
            "genre": [
                { "genreName": "推荐作品" },
                { "genreName": "", "genreName_ja": "美尻" }
            ],
            "star": [
                { "starId": "dnyaqln", "starName": "大海まりん", "avatarUrl": "https://file.netcdn.space/a.webp" }
            ]
        });
        let r = movie_json_to_result(&m, "SMBD-050").unwrap();
        assert_eq!(r.code, "SMBD-050");
        assert_eq!(r.source, "avsox");
        assert!(r.is_uncensored);
        assert_eq!(r.title, "S Model 50 : 大海まりん (ブルーレイディスク版)");
        assert_eq!(r.premiered, "2012-03-05");
        assert_eq!(r.duration, "110分钟");
        assert_eq!(r.studio, "スーパーモデルメディア");
        assert_eq!(r.label, "S Model - Blu-ray");
        assert_eq!(r.tags, "推荐作品, 美尻");
        assert_eq!(r.actors, "大海まりん");
        assert_eq!(r.actor_avatars[0].avatar_url, "https://file.netcdn.space/a.webp");
        assert!(r.actor_avatars[0].star_code.is_empty());
        assert_eq!(r.cover_url, "https://file.netcdn.space/storage/ave/archive/bigcover/dvd1smbd-50.jpg");
        // 样图换到 AVE 官网图床（接口给的 netcdn 地址已失效）
        assert_eq!(
            r.thumbs,
            vec!["https://imgs02.aventertainments.com/archive/vodimages/screenshot/large/SMBD-50/001.webp"]
        );
        assert_eq!(r.page_url, "https://avsox.click/cn/movie/jnoqmdn");
        assert!(r.plot.is_empty());
    }

    #[test]
    fn rewrite_sample_url_only_touches_dead_ave_mirror() {
        assert_eq!(
            rewrite_sample_url("https://file.netcdn.space/ave/vodimages/screenshot/small/CWPBD-45/012.jpg"),
            "https://imgs02.aventertainments.com/archive/vodimages/screenshot/small/CWPBD-45/012.webp"
        );
        // 其它厂牌 / 已是官网地址：原样
        assert_eq!(
            rewrite_sample_url("https://file.netcdn.space/storage/heyzo/contents/3000/2856/gallery/001.jpg"),
            "https://file.netcdn.space/storage/heyzo/contents/3000/2856/gallery/001.jpg"
        );
        assert_eq!(
            rewrite_sample_url("https://imgs02.aventertainments.com/archive/vodimages/screenshot/large/SMBD-50/001.webp"),
            "https://imgs02.aventertainments.com/archive/vodimages/screenshot/large/SMBD-50/001.webp"
        );
    }

    #[test]
    fn movie_json_falls_back_to_ja_title_and_rejects_empty() {
        let m = json!({ "movieId": "x", "title": "", "title_ja": "日文标题", "posterLarge": "" });
        assert_eq!(movie_json_to_result(&m, "N0698").unwrap().title, "日文标题");
        assert!(movie_json_to_result(&json!({ "movieId": "x" }), "N0698").is_none());
    }

    #[test]
    fn extract_detail_url_prefers_code_match() {
        let html = r#"
            <a class="movie-box" href="/cn/movie/aaa"><span>OTHER-001</span></a>
            <a class="movie-box" href="/cn/movie/bbb"><span>HEYZO-1234 标题</span></a>
        "#;
        let url = Avsox.extract_detail_url(html, "HEYZO-1234").unwrap();
        assert_eq!(url, "https://avsox.click/cn/movie/bbb");
    }

    #[test]
    fn extract_detail_url_keeps_d2pass_separator() {
        // 加勒比 110615-001 与一本道 110615_001 同时在列：按原样番号精确匹配，不混淆
        let html = r#"
            <a class="movie-box" href="/cn/movie/carib"><span>110615-001 加勒比</span></a>
            <a class="movie-box" href="/cn/movie/1pon"><span>110615_001 一本道</span></a>
        "#;
        assert_eq!(
            Avsox.extract_detail_url(html, "110615_001").unwrap(),
            "https://avsox.click/cn/movie/1pon"
        );
        assert_eq!(
            Avsox.extract_detail_url(html, "110615-001").unwrap(),
            "https://avsox.click/cn/movie/carib"
        );
        // 精确未命中：D2Pass 番号不做模糊/首项回退（会拿到别家同号影片）
        assert!(Avsox.extract_detail_url(html, "110615_002").is_none());
    }

    #[test]
    fn parse_extracts_moo_family_fields() {
        // moo 家族详情页结构（与 javbus 类似），验证选择器拼写正确
        let html = r#"
            <a class="bigImage" href="/cover/big.jpg"><img src="/cover/small.jpg"></a>
            <h3>HEYZO-1234 测试标题</h3>
            <div class="info">
              <p><span class="header">發行日期:</span> 2020-01-01</p>
              <p><span class="header">長度:</span> 60分鐘</p>
              <p><span class="header">系列:</span> 测试系列</p>
            </div>
            <span class="genre"><a href="/cn/genre/xxx">巨乳</a></span>
            <a class="avatar-box"><div class="photo-info"><span>测试演员</span></div></a>
            <div id="sample-waterfall"><a class="sample-box" href="/preview/1.jpg"></a></div>
        "#;
        let r = Avsox.parse(html, "HEYZO-1234").unwrap();
        assert!(r.is_uncensored);
        assert_eq!(r.source, "avsox");
        assert_eq!(r.title, "测试标题");
        assert_eq!(r.premiered, "2020-01-01");
        assert_eq!(r.duration, "60分钟");
        assert_eq!(r.label, "测试系列");
        assert_eq!(r.cover_url, "https://avsox.click/cover/big.jpg");
        assert_eq!(r.actors, "测试演员");
        assert_eq!(r.tags, "巨乳");
        assert_eq!(r.thumbs, vec!["https://avsox.click/preview/1.jpg"]);
    }
}
