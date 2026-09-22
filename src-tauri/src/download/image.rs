//! 图片下载基础模块
//!
//! 提供统一的图片下载能力，包括：
//! - 单张图片下载
//! - 批量并发图片下载（信号量控制并发数）
//!
//! 所有需要下载图片的模块（media_assets、scraper 等）都应使用此模块，
//! 避免重复实现下载逻辑。
//! 使用 wreq 客户端（Chrome TLS 指纹），统一反爬策略。

use std::path::Path;
use std::sync::Arc;

use wreq::Client as HttpClient;

/// 默认最大并发下载数
const DEFAULT_MAX_CONCURRENT: usize = 5;

/// 创建默认的 wreq HTTP 客户端（Chrome TLS 指纹 + 代理）
fn default_client() -> Result<HttpClient, String> {
    crate::resource_scrape::fingerprint_client::shared_client()
}

/// 校验下载到的字节是可用图片，否则报错、不落盘。
///
/// - HTML/404 页、空数据等无法解码 → 无效（否则会存成打不开的假 .jpg）。
/// - **AVIF 明确拒绝**：`image` crate 无 AVIF 解码器（裁不出竖版海报、生成不了网格缩略图），
///   Jellyfin 的 Skia 同样不支持；jav.place 等站点的封面 CDN 只回 AVIF，
///   此前被原样写成 `.jpg`，媒体库里表现为整片无海报。拒绝后由调用方换下一个候选源。
fn ensure_supported_image(bytes: &[u8], source: &str) -> Result<(), String> {
    let short = &source[..source.len().min(100)];
    if bytes.is_empty() {
        return Err(format!("下载的数据为空: {}", short));
    }
    if matches!(image::guess_format(bytes), Ok(image::ImageFormat::Avif)) {
        return Err(format!("图片为 AVIF 格式，无法解码，已跳过: {}", short));
    }
    if !crate::media::artwork::is_valid_image_bytes(bytes, 0) {
        return Err(format!("下载内容不是有效图片: {}", short));
    }
    Ok(())
}

/// 下载单张图片并保存到指定路径（下载后先校验为可解码图片，见 [`ensure_supported_image`]）
pub async fn download_image(
    client: &HttpClient,
    url: &str,
    save_path: &Path,
) -> Result<String, String> {
    let bytes = crate::resource_scrape::fingerprint_client::fetch_bytes(client, url).await?;
    ensure_supported_image(&bytes, url)?;

    tokio::fs::write(save_path, &bytes)
        .await
        .map_err(|e| format!("写入文件失败: {}", e))?;

    Ok(save_path.to_string_lossy().to_string())
}

/// 将图片 URL 保存到指定文件路径（落点完全由调用方决定）。
///
/// 支持三种来源：
/// - `data:image/...;base64,...` — 解码 base64 写入
/// - `http(s)://...` — HTTP 下载
/// - 本地文件路径 — 直接复制（搜索阶段代理缓存的结果）
///
/// 三种来源都先校验为可解码图片（AVIF 视为不可用），不合格返回错误、不落盘。
/// 空 URL 返回空串（视为无图，非错误）。
pub async fn save_image_url_to(
    url: &str,
    save_path: &Path,
    client: Option<&HttpClient>,
) -> Result<String, String> {
    let url = url.trim();
    if url.is_empty() {
        return Ok(String::new());
    }

    // 处理 data URL（base64 编码的图片数据）
    if url.starts_with("data:") {
        return save_data_url_to_file(url, save_path);
    }

    // 处理 HTTP URL
    if url.starts_with("http://") || url.starts_with("https://") {
        let owned_client;
        let client = match client {
            Some(c) => c,
            None => {
                owned_client = default_client()?;
                &owned_client
            }
        };
        return download_image(client, url, save_path).await;
    }

    // 处理本地缓存文件路径（搜索阶段代理下载的临时文件）
    let source_path = Path::new(url);
    if source_path.exists() {
        let bytes = std::fs::read(source_path).map_err(|e| format!("读取图片缓存文件失败: {}", e))?;
        ensure_supported_image(&bytes, url)?;
        std::fs::write(save_path, &bytes).map_err(|e| format!("复制图片缓存文件失败: {}", e))?;
        return Ok(save_path.to_string_lossy().to_string());
    }

    Err(format!("无法识别的图片 URL 格式: {}", &url[..url.len().min(100)]))
}

/// 将 data URL（base64）解码并保存为文件
fn save_data_url_to_file(data_url: &str, save_path: &Path) -> Result<String, String> {
    // 格式: data:image/jpeg;base64,/9j/4AAQ...
    let base64_data = data_url
        .find(",")
        .map(|i| &data_url[i + 1..])
        .ok_or("无效的 data URL 格式")?;

    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_data)
        .map_err(|e| format!("base64 解码失败: {}", e))?;
    ensure_supported_image(&bytes, "data:…")?;

    let mut file =
        std::fs::File::create(save_path).map_err(|e| format!("创建文件失败: {}", e))?;

    std::io::Write::write_all(&mut file, &bytes)
        .map_err(|e| format!("写入文件失败: {}", e))?;

    Ok(save_path.to_string_lossy().to_string())
}

/// 批量并发下载缩略图
///
/// 使用信号量控制并发数，所有图片同时发起但最多 N 个并行下载。
/// 单张下载失败不会中断整个过程。
pub async fn download_images_batch(
    thumb_urls: &[String],
    save_dir: &Path,
    filename_prefix: &str,
    client: Option<&HttpClient>,
    max_concurrent: Option<usize>,
) -> Result<Vec<String>, String> {
    if thumb_urls.is_empty() {
        return Ok(Vec::new());
    }

    std::fs::create_dir_all(save_dir)
        .map_err(|e| format!("创建目录失败: {}", e))?;

    let owned_client;
    let client = match client {
        Some(c) => c,
        None => {
            owned_client = default_client()?;
            &owned_client
        }
    };

    let concurrent = max_concurrent.unwrap_or(DEFAULT_MAX_CONCURRENT);
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrent));
    let client = Arc::new(client.clone());

    let tasks: Vec<(usize, String)> = thumb_urls
        .iter()
        .enumerate()
        .filter(|(_, url)| !url.trim().is_empty())
        .map(|(i, url)| (i, url.clone()))
        .collect();

    let prefix = filename_prefix.to_string();
    let handles: Vec<_> = tasks
        .into_iter()
        .map(|(index, url)| {
            let sem = semaphore.clone();
            let client = client.clone();
            let filename = format!("{}_{:03}.jpg", prefix, index + 1);
            let save_path = save_dir.join(&filename);

            tokio::spawn(async move {
                let _permit = sem
                    .acquire()
                    .await
                    .map_err(|e| format!("获取信号量失败: {}", e))?;
                download_image(&client, &url, &save_path).await
            })
        })
        .collect();

    let mut saved_paths = Vec::new();
    for handle in handles {
        match handle.await {
            Ok(Ok(path)) => saved_paths.push(path),
            Ok(Err(e)) => {
                log::warn!("[image] event=thumb_download_failed error={}", e);
            }
            Err(e) => {
                log::warn!("[image] event=thumb_download_task_failed error={}", e);
            }
        }
    }

    Ok(saved_paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_save_image_url_to_empty_url() {
        let dir = std::env::temp_dir();
        let result = save_image_url_to("", &dir.join("none.jpg"), None).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_save_image_url_to_local_copy() {
        let dir = std::env::temp_dir();
        let src = dir.join(format!("javm-img-src-{}.png", std::process::id()));
        // 真实可解码的图片才允许落盘
        image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3])).save(&src).unwrap();
        let dst = dir.join(format!("javm-img-dst-{}.jpg", std::process::id()));

        let result = save_image_url_to(&src.to_string_lossy(), &dst, None).await;
        assert!(result.is_ok());
        assert!(dst.exists());

        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&dst);
    }

    #[tokio::test]
    async fn test_save_image_url_to_rejects_non_image_and_avif() {
        let dir = std::env::temp_dir();
        let pid = std::process::id();
        // 非图片内容（防盗链常回的 HTML）
        let html = dir.join(format!("javm-img-html-{}.bin", pid));
        std::fs::write(&html, b"<html>403</html>").unwrap();
        let dst = dir.join(format!("javm-img-reject-{}.jpg", pid));
        let err = save_image_url_to(&html.to_string_lossy(), &dst, None).await.unwrap_err();
        assert!(err.contains("不是有效图片"), "{}", err);
        assert!(!dst.exists(), "无效图片不应落盘");

        // AVIF 文件头（ftypavif）：无解码器，且 Jellyfin 也不支持，须拒绝而非存成假 jpg
        let avif = dir.join(format!("javm-img-avif-{}.bin", pid));
        let mut bytes = b"\0\0\0\x20ftypavif\0\0\0\0avifmif1miafMA1B".to_vec();
        bytes.extend_from_slice(&[0u8; 64]);
        std::fs::write(&avif, &bytes).unwrap();
        let err = save_image_url_to(&avif.to_string_lossy(), &dst, None).await.unwrap_err();
        assert!(err.contains("AVIF"), "{}", err);
        assert!(!dst.exists());

        let _ = std::fs::remove_file(&html);
        let _ = std::fs::remove_file(&avif);
    }

    #[tokio::test]
    async fn test_download_images_batch_empty_urls() {
        let dir = PathBuf::from("/tmp/test");
        let result = download_images_batch(&[], &dir, "thumb", None, None).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_thumb_filename_generation() {
        let filenames: Vec<String> = (1..=5)
            .map(|i| format!("thumb_{:03}.jpg", i))
            .collect();
        assert_eq!(filenames[0], "thumb_001.jpg");
        assert_eq!(filenames[4], "thumb_005.jpg");
    }
}
