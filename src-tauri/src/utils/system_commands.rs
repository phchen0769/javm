use std::time::Duration;
use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder};
use uuid::Uuid;

const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";

/// 检查物理坐标 (x, y) 是否在任一显示器的可见区域内
pub fn is_position_visible_on_monitors(window: &tauri::WebviewWindow, x: f64, y: f64) -> bool {
    if let Ok(monitors) = window.available_monitors() {
        for m in monitors {
            let pos = m.position();
            let size = m.size();
            if (x as i32) < pos.x + size.width as i32 - 100
                && (x as i32) + 100 > pos.x
                && (y as i32) < pos.y + size.height as i32 - 100
                && (y as i32) + 100 > pos.y
            {
                return true;
            }
        }
        false
    } else {
        true // 获取失败则假定可见
    }
}

pub async fn open_in_explorer(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let path_obj = std::path::Path::new(&path);
        if path_obj.is_dir() {
            std::process::Command::new("explorer")
                .arg(&path)
                .spawn()
                .map_err(|e| e.to_string())?;
        } else {
            std::process::Command::new("explorer")
                .arg("/select,")
                .arg(&path)
                .spawn()
                .map_err(|e| e.to_string())?;
        }
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        let path_obj = std::path::Path::new(&path);
        if path_obj.is_dir() {
            std::process::Command::new("xdg-open")
                .arg(&path)
                .spawn()
                .map_err(|e| e.to_string())?;
        } else if let Some(parent) = path_obj.parent() {
            std::process::Command::new("xdg-open")
                .arg(parent)
                .spawn()
                .map_err(|e| e.to_string())?;
        } else {
            std::process::Command::new("xdg-open")
                .arg(&path)
                .spawn()
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Open file with default system player/application
///
/// # Arguments
/// * `path` - Path to the file to open
///
/// # Platform Support
/// - Windows: Uses `explorer` (opens with default application)
/// - macOS: Uses `open`
/// - Linux: Uses `xdg-open`
pub async fn open_with_player(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn should_open_ts_with_system_player(video_url: &str, is_hls: bool) -> bool {
    if is_hls {
        return false;
    }

    let lower = video_url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return false;
    }

    lower.ends_with(".ts") || lower.ends_with(".m2ts")
}

pub async fn open_video_player_window(
    app: AppHandle,
    video_url: String,
    title: String,
    is_hls: bool,
) -> Result<(), String> {
    if should_open_ts_with_system_player(&video_url, is_hls) {
        return open_with_player(video_url).await;
    }

    // 使用 url crate 的 query_pairs_mut 对参数进行编码
    let mut temp_url = url::Url::parse("http://x/video-player").unwrap();
    temp_url.query_pairs_mut()
        .append_pair("url", &video_url)
        .append_pair("title", &title)
        .append_pair("is_hls", &is_hls.to_string());
    let url_str = format!("/video-player?{}", temp_url.query().unwrap_or_default());

    spawn_player_window(app, url_str).await
}

/// 打开内置播放器并载入分段播放列表（播完自动续下一段）。列表 ≤1 条时回退单文件播放。
/// `paths` 为本地文件原始路径，前端自行 `convertFileSrc`。
pub async fn open_video_playlist_window(
    app: AppHandle,
    paths: Vec<String>,
    title: String,
) -> Result<(), String> {
    if paths.len() <= 1 {
        let single = paths.into_iter().next().unwrap_or_default();
        return open_video_player_window(app, single, title, false).await;
    }

    let playlist_json = serde_json::to_string(&paths).map_err(|e| e.to_string())?;

    let mut temp_url = url::Url::parse("http://x/video-player").unwrap();
    temp_url.query_pairs_mut()
        .append_pair("url", &paths[0])
        .append_pair("title", &title)
        .append_pair("is_hls", "false")
        .append_pair("playlist", &playlist_json)
        .append_pair("index", "0");
    let url_str = format!("/video-player?{}", temp_url.query().unwrap_or_default());

    spawn_player_window(app, url_str).await
}

/// 依据 `/video-player?...` 相对路径创建播放器窗口（含尺寸/位置/置顶等设置恢复）。
async fn spawn_player_window(app: AppHandle, url_str: String) -> Result<(), String> {
    let window_label = format!("video_player_{}", Uuid::new_v4().simple());
    let url = WebviewUrl::App(url_str.into());

    use crate::settings::get_settings;

    // 获取配置
    let settings = get_settings(app.clone()).await.unwrap_or_default();
    let vp_settings = settings.video_player;

    let builder = WebviewWindowBuilder::new(&app, window_label, url)
        .title("视频播放")
        .min_inner_size(400.0, 300.0)
        .always_on_top(vp_settings.always_on_top)
        .visible(false);

    // macOS：使用系统原生窗口按钮（左上角交通灯），标题栏透明覆盖在视频上方
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);

    // 其他平台：无边框窗口，窗口按钮由前端自绘（右上角）
    #[cfg(not(target_os = "macos"))]
    let builder = builder.decorations(false);

    let window = builder.build().map_err(|e| e.to_string())?;

    if let (Some(w), Some(h)) = (vp_settings.width, vp_settings.height) {
        let _ = window.set_size(tauri::PhysicalSize::new(w as u32, h as u32));
    } else {
        let _ = window.set_size(tauri::LogicalSize::new(800.0, 600.0));
    }

    let mut position_set = false;
    if let (Some(x), Some(y)) = (vp_settings.x, vp_settings.y) {
        if is_position_visible_on_monitors(&window, x, y) {
            let _ = window.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
            position_set = true;
        }
    }

    if !position_set {
        let _ = window.center();
    }

    let _ = window.show();

    Ok(())
}

/// 代理 HLS 请求，绕过浏览器 CORS 限制
/// 返回 (base64_data, content_type)
pub async fn proxy_hls_request(
    url: String,
    referer: Option<String>,
) -> Result<(String, String), String> {
    // 防 SSRF：仅允许 http/https，且拒绝本机/内网地址，避免前端传入任意 URL
    // 探测内网服务（如 127.0.0.1、169.254.169.254 元数据端点）。
    validate_proxy_target(&url)?;

    let client = crate::utils::proxy::apply_proxy_auto(
        wreq::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(DEFAULT_USER_AGENT),
    )
    .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?
    .build()
    .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let mut req = client.get(&url);

    if let Some(ref r) = referer {
        req = req.header("Referer", r);
        req = req.header("Origin", r.trim_end_matches('/'));
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("代理请求失败: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("代理请求失败，HTTP 状态码: {}", resp.status()));
    }

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取响应数据失败: {}", e))?;

    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

    Ok((b64, content_type))
}

/// 校验代理目标 URL：仅允许 http/https，拒绝本机/内网地址（防 SSRF）。
fn validate_proxy_target(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|_| "无效的 URL".to_string())?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("不支持的协议: {}", other)),
    }
    let host = parsed.host_str().ok_or_else(|| "URL 缺少主机".to_string())?;
    if host.eq_ignore_ascii_case("localhost") {
        return Err("不允许访问本机地址".to_string());
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if is_internal_ip(&ip) {
            return Err("不允许访问内网/本机地址".to_string());
        }
    }
    Ok(())
}

/// 判断 IP 是否为本机/内网/链路本地等不应被代理访问的地址。
fn is_internal_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local() // 含 169.254.169.254 云元数据
                || v4.is_unspecified()
                || v4.is_broadcast()
        }
        std::net::IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_internal_ip(&std::net::IpAddr::V4(v4));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7 唯一本地
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10 链路本地
        }
    }
}
