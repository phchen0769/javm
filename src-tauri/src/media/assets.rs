//! 视频媒体资源管理模块
//!
//! 统一处理视频相关的媒体资源操作，包括：
//! - NFO 元数据文件保存
//! - 封面图片下载/截取保存
//! - 视频帧截取（ffmpeg）
//! - 预览截图保存
//! - 文件回滚

use std::fs;
use std::path::{Path, PathBuf};

use crate::nfo::generator::NfoGenerator;
use crate::resource_scrape::types::ScrapeMetadata;

const EXTRAFANART_DIR_NAME: &str = "extrafanart";
const SUBTITLE_EXTENSIONS: &[&str] = &[
    "srt", "ass", "ssa", "vtt", "sub", "idx", "smi", "sup", "sbv", "dfxp", "ttml",
    "scc", "usf",
];

#[derive(Debug, Clone)]
pub struct RelocatedVideoAssets {
    pub original_video_path: String,
    pub video_path: String,
    pub dir_path: String,
    pub poster: Option<String>,
    pub thumb: Option<String>,
    pub fanart: Option<String>,
    /// 分段影片归入组目录时随之搬入的同组其它段，供调用方同步写库；非分段影片为空
    pub moved_siblings: Vec<MovedStackSibling>,
}

/// 随分段影片一起搬进组目录的同组其它段（原路径 → 新路径）
#[derive(Debug, Clone)]
pub struct MovedStackSibling {
    pub original_video_path: String,
    pub video_path: String,
}

// ============================================================
// NFO 元数据
// ============================================================

/// 统一的 NFO 保存逻辑：检查本地封面是否存在，然后调用 NfoGenerator 生成 NFO 文件
///
/// 供 queue_manager、commands 等模块复用，避免重复实现。
pub fn save_nfo_for_video(video_path: &str, metadata: &ScrapeMetadata) -> Result<(), String> {
    let path = Path::new(video_path);
    let parent_dir = path.parent().ok_or("无效的视频路径")?;
    let file_stem = path
        .file_stem()
        .ok_or("无效的视频文件名")?
        .to_string_lossy()
        .to_string();

    save_nfo_to(parent_dir, &file_stem, metadata)
}

/// 将 NFO 保存到指定目录，文件名为 `<stem>.nfo`；按同目录已存在的标准图集文件
/// （`<stem>-poster/fanart/thumb.*`）引用相对文件名。
///
/// 供独立目录模式直接写入 `<root>/<番号 标题>/<番号>.nfo`。
pub fn save_nfo_to(dir: &Path, stem: &str, metadata: &ScrapeMetadata) -> Result<(), String> {
    let generator = NfoGenerator::new();
    let artwork = detect_local_artwork(dir, stem);
    let nfo_path = dir.join(format!("{}.nfo", stem));
    generator.save_to(metadata, &nfo_path, &artwork).map(|_| ())
}

/// 探测同目录已存在的标准图集文件，返回 NFO 引用的相对文件名（poster/fanart/thumb）。
pub fn detect_local_artwork(dir: &Path, stem: &str) -> crate::nfo::generator::NfoArtwork {
    crate::nfo::generator::NfoArtwork {
        poster: detect_artwork_filename(dir, stem, crate::media::artwork::POSTER_SUFFIX),
        fanart: detect_artwork_filename(dir, stem, crate::media::artwork::FANART_SUFFIX),
        thumb: detect_artwork_filename(dir, stem, crate::media::artwork::THUMB_SUFFIX),
    }
}

fn detect_artwork_filename(dir: &Path, stem: &str, suffix: &str) -> Option<String> {
    ["jpg", "jpeg", "png", "webp"]
        .iter()
        .map(|ext| format!("{}-{}.{}", stem, suffix, ext))
        .find(|name| dir.join(name).exists())
}

pub fn has_same_named_parent_dir(video_path: &Path) -> bool {
    let Some(parent_dir) = video_path.parent() else {
        return false;
    };
    let Some(parent_name) = parent_dir.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(file_stem) = video_path.file_stem().and_then(|name| name.to_str()) else {
        return false;
    };

    parent_name.eq_ignore_ascii_case(file_stem)
}

/// 分段影片（`SIVR-015-2.mp4`）所在目录是否已属于该分段组：目录名就是组基名（`SIVR-015`）
/// 或组内某一段（`SIVR-015-1`）。此时不再按段名另建子目录，否则同组各段会被拆到不同目录
/// （`SIVR-015-1/SIVR-015-1.mp4` + `SIVR-015-1/SIVR-015-2/SIVR-015-2.mp4`）。
fn is_stack_part_in_grouped_dir(video_path: &Path) -> bool {
    use crate::utils::designation_recognizer::{dir_belongs_to_stack, parse_stack_part};

    let Some(stem) = video_path.file_stem().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some((base, _)) = parse_stack_part(stem) else {
        return false;
    };
    video_path
        .parent()
        .and_then(|dir| dir.file_name())
        .and_then(|name| name.to_str())
        .is_some_and(|parent_name| dir_belongs_to_stack(parent_name, &base))
}

fn is_subtitle_suffix_separator(ch: char) -> bool {
    matches!(ch, '.' | '_' | '-' | ' ' | '[' | '(')
}

/// 候选文件名 stem（已小写）是否与视频 stem（已小写）匹配：相等，或以「stem+分隔符」为前缀
/// （`ABC-123.zh.srt`、`ABC-123-eng.ass`、`ABC-123 [chs].vtt`）。
fn subtitle_stem_matches(video_stem_lower: &str, candidate_stem_lower: &str) -> bool {
    candidate_stem_lower == video_stem_lower
        || candidate_stem_lower
            .strip_prefix(video_stem_lower)
            .is_some_and(|suffix| suffix.chars().next().is_some_and(is_subtitle_suffix_separator))
}

/// 从目录项列表中收集字幕文件的 stem（小写），供同目录多个视频复用一次 read_dir 的结果。
pub fn collect_subtitle_stems(entries: &[std::fs::DirEntry]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|entry| {
            let path = entry.path();
            let extension = path.extension().and_then(|ext| ext.to_str())?;
            if !SUBTITLE_EXTENSIONS.iter().any(|item| item.eq_ignore_ascii_case(extension)) {
                return None;
            }
            path.file_stem().and_then(|name| name.to_str()).map(|s| s.to_ascii_lowercase())
        })
        .collect()
}

/// 给定目录内字幕 stem 列表（见 [`collect_subtitle_stems`]），判断视频 stem 是否有匹配字幕。
pub fn stem_has_matching_subtitle(subtitle_stems_lower: &[String], video_stem: &str) -> bool {
    let video_stem_lower = video_stem.to_ascii_lowercase();
    subtitle_stems_lower
        .iter()
        .any(|candidate| subtitle_stem_matches(&video_stem_lower, candidate))
}

/// 目录内是否存在与给定 stem 匹配的字幕文件（任意语言/扩展名）。
///
/// stem 由 `resolve_existing_asset_dir` 给出（跟随视频=视频文件名，独立目录=番号）。
/// 会做一次 read_dir，只应在扫描入库、字幕回填等低频路径调用，不要在列表加载时逐视频调用。
pub fn dir_has_matching_subtitle(dir: &Path, stem: &str) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    let entries: Vec<std::fs::DirEntry> = entries.flatten().collect();
    stem_has_matching_subtitle(&collect_subtitle_stems(&entries), stem)
}

fn is_matching_subtitle_file(video_path: &Path, candidate: &Path) -> bool {
    let Some(video_parent) = video_path.parent() else {
        return false;
    };
    let Some(candidate_parent) = candidate.parent() else {
        return false;
    };
    if video_parent != candidate_parent {
        return false;
    }

    let Some(extension) = candidate.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    if !SUBTITLE_EXTENSIONS
        .iter()
        .any(|item| item.eq_ignore_ascii_case(extension))
    {
        return false;
    }

    let Some(video_stem) = video_path.file_stem().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(candidate_stem) = candidate.file_stem().and_then(|name| name.to_str()) else {
        return false;
    };

    let video_stem_lower = video_stem.to_ascii_lowercase();
    let candidate_stem_lower = candidate_stem.to_ascii_lowercase();

    candidate_stem_lower == video_stem_lower
        || candidate_stem_lower
            .strip_prefix(&video_stem_lower)
            .is_some_and(|suffix| {
                suffix
                    .chars()
                    .next()
                    .is_some_and(is_subtitle_suffix_separator)
            })
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else {
            fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}

fn move_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(src, dst)?;
            fs::remove_file(src)?;
            Ok(())
        }
    }
}

fn move_dir(src: &Path, dst: &Path) -> Result<(), String> {
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            copy_dir_recursive(src, dst).map_err(|e| format!("复制目录失败: {}", e))?;
            fs::remove_dir_all(src).map_err(|e| format!("删除原目录失败: {}", e))?;
            Ok(())
        }
    }
}

fn resolve_asset_source(video_path: &Path, explicit_path: Option<&str>, suffix: &str) -> Option<PathBuf> {
    // 仅接受与视频同级的图：独立元数据目录里的图不随视频移动/重命名搬走，留在独立目录。
    let video_parent = video_path.parent();
    explicit_path
        .map(PathBuf::from)
        .filter(|path| path.exists() && path.is_file() && path.parent() == video_parent)
        .or_else(|| find_sibling_artwork(video_path, suffix).map(PathBuf::from))
}

fn move_optional_asset(source: Option<PathBuf>, target_dir: &Path, label: &str) -> Option<String> {
    let source = source?;
    let file_name = match source.file_name() {
        Some(file_name) => file_name,
        None => {
            log::error!(
                "[media_assets] event=move_optional_asset_invalid_filename label={} source={}",
                label,
                source.display()
            );
            return None;
        }
    };

    let target = target_dir.join(file_name);
    if target.exists() && !source.exists() {
        return Some(target.to_string_lossy().to_string());
    }

    if source == target {
        return Some(target.to_string_lossy().to_string());
    }

    if !source.exists() {
        return None;
    }

    match move_file(&source, &target) {
        Ok(()) => Some(target.to_string_lossy().to_string()),
        Err(error) => {
            log::error!(
                "[media_assets] event=move_optional_asset_failed label={} source={} target={} error={}",
                label,
                source.display(),
                target.display(),
                error
            );
            None
        }
    }
}

fn move_matching_subtitle_files(video_path: &Path, target_dir: &Path) {
    let Some(parent_dir) = video_path.parent() else {
        return;
    };

    let Ok(entries) = fs::read_dir(parent_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let candidate = entry.path();
        if !is_matching_subtitle_file(video_path, &candidate) {
            continue;
        }

        let Some(file_name) = candidate.file_name() else {
            continue;
        };
        let target = target_dir.join(file_name);
        if let Err(error) = move_file(&candidate, &target) {
            log::error!(
                "[media_assets] event=move_subtitle_failed source={} target={} error={}",
                candidate.display(),
                target.display(),
                error
            );
        }
    }
}

#[derive(Debug, Clone)]
struct PendingRenameOperation {
    source: PathBuf,
    target: PathBuf,
    is_dir: bool,
}

fn is_same_path_for_fs(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.to_string_lossy().eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

fn sanitize_title_for_path(title: &str) -> Result<String, String> {
    let sanitized = title
        .trim()
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect::<String>()
        .trim()
        .trim_end_matches(['.', ' '])
        .to_string();

    if sanitized.is_empty() {
        return Err("标题为空或包含非法字符，无法作为文件名".to_string());
    }

    Ok(sanitized)
}

fn remap_path_after_dir_rename(path: &Path, old_dir: &Path, new_dir: &Path) -> PathBuf {
    match path.strip_prefix(old_dir) {
        Ok(relative) => new_dir.join(relative),
        Err(_) => path.to_path_buf(),
    }
}

fn build_artwork_target_path(source: &Path, target_dir: &Path, new_stem: &str, suffix: &str) -> Option<PathBuf> {
    let extension = source.extension()?.to_str()?;
    Some(target_dir.join(format!("{}-{}.{}", new_stem, suffix, extension)))
}

fn queue_optional_file_rename(
    operations: &mut Vec<PendingRenameOperation>,
    source: Option<PathBuf>,
    target: Option<PathBuf>,
) {
    let (Some(source), Some(target)) = (source, target) else {
        return;
    };

    if is_same_path_for_fs(&source, &target) {
        return;
    }

    operations.push(PendingRenameOperation {
        source,
        target,
        is_dir: false,
    });
}

fn queue_matching_subtitle_renames(
    operations: &mut Vec<PendingRenameOperation>,
    video_path: &Path,
    old_dir: &Path,
    target_dir: &Path,
    new_stem: &str,
    rename_parent_dir: bool,
) {
    let Some(parent_dir) = video_path.parent() else {
        return;
    };

    let Some(video_stem) = video_path.file_stem().and_then(|name| name.to_str()) else {
        return;
    };

    let Ok(entries) = fs::read_dir(parent_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let source = entry.path();
        if !is_matching_subtitle_file(video_path, &source) {
            continue;
        }

        let extension = match source.extension().and_then(|ext| ext.to_str()) {
            Some(extension) => extension.to_string(),
            None => continue,
        };

        let candidate_stem = match source.file_stem().and_then(|name| name.to_str()) {
            Some(stem) => stem,
            None => continue,
        };

        let suffix = candidate_stem
            .strip_prefix(video_stem)
            .map(str::to_string)
            .or_else(|| {
                let video_stem_lower = video_stem.to_ascii_lowercase();
                let candidate_stem_lower = candidate_stem.to_ascii_lowercase();
                candidate_stem_lower
                    .strip_prefix(&video_stem_lower)
                    .map(|rest| candidate_stem[candidate_stem.len() - rest.len()..].to_string())
            })
            .unwrap_or_default();

        let remapped_source = if rename_parent_dir {
            remap_path_after_dir_rename(&source, old_dir, target_dir)
        } else {
            source.clone()
        };
        let target = target_dir.join(format!("{}{}.{}", new_stem, suffix, extension));

        if is_same_path_for_fs(&remapped_source, &target) {
            continue;
        }

        operations.push(PendingRenameOperation {
            source: remapped_source,
            target,
            is_dir: false,
        });
    }
}

fn rollback_rename_operations(completed: &[PendingRenameOperation]) {
    for operation in completed.iter().rev() {
        if !operation.target.exists() {
            continue;
        }

        if let Some(parent) = operation.source.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let rollback_result = if operation.is_dir {
            move_dir(&operation.target, &operation.source)
        } else {
            move_file(&operation.target, &operation.source)
                .map_err(|error| format!("回滚文件失败: {}", error))
        };

        if let Err(error) = rollback_result {
            log::error!(
                "[media_assets] event=rollback_rename_failed source={} target={} error={}",
                operation.source.display(),
                operation.target.display(),
                error
            );
        }
    }
}

fn execute_rename_operations(operations: &[PendingRenameOperation]) -> Result<(), String> {
    let mut completed = Vec::new();

    for operation in operations {
        if let Some(parent) = operation.target.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
        }

        if operation.target.exists() && !is_same_path_for_fs(&operation.source, &operation.target) {
            rollback_rename_operations(&completed);
            return Err(format!("目标已存在，无法重命名: {}", operation.target.display()));
        }

        let result = if operation.is_dir {
            move_dir(&operation.source, &operation.target)
        } else {
            move_file(&operation.source, &operation.target)
                .map_err(|e| format!("重命名文件失败: {}", e))
        };

        if let Err(error) = result {
            rollback_rename_operations(&completed);
            return Err(format!(
                "重命名失败: {} -> {}: {}",
                operation.source.display(),
                operation.target.display(),
                error
            ));
        }

        completed.push(operation.clone());
    }

    Ok(())
}

pub fn rename_video_assets_with_title(
    video_path: &str,
    new_title: &str,
    poster: Option<&str>,
    thumb: Option<&str>,
    fanart: Option<&str>,
) -> Result<Option<RelocatedVideoAssets>, String> {
    let video_path_obj = Path::new(video_path);
    if !video_path_obj.exists() {
        return Err("源视频文件不存在".to_string());
    }

    let old_dir = video_path_obj.parent().ok_or("无效的视频路径")?;
    let old_stem = video_path_obj
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("无效的视频文件名")?;
    // 分段影片不按标题改名：只改当前段会与同组其它段的基名脱节，合集随即拆散
    if crate::utils::designation_recognizer::parse_stack_part(old_stem).is_some() {
        log::info!(
            "[media_assets] event=rename_skipped_stack_part video_path={} new_title={}",
            video_path,
            new_title
        );
        return Ok(None);
    }
    let new_stem = sanitize_title_for_path(new_title)?;
    let current_parent_name = old_dir.file_name().and_then(|name| name.to_str());
    let already_in_target_parent = current_parent_name
        .is_some_and(|name| name.eq_ignore_ascii_case(&new_stem));

    let rename_parent_dir = old_dir
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(old_stem));

    let target_dir = if already_in_target_parent {
        old_dir.to_path_buf()
    } else if rename_parent_dir {
        let parent_of_parent = old_dir.parent().ok_or("无效的父目录")?;
        parent_of_parent.join(&new_stem)
    } else {
        old_dir.join(&new_stem)
    };

    let current_video_path = if rename_parent_dir {
        remap_path_after_dir_rename(video_path_obj, old_dir, &target_dir)
    } else {
        video_path_obj.to_path_buf()
    };

    let new_file_name = match video_path_obj.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if !ext.is_empty() => format!("{}.{}", new_stem, ext),
        _ => new_stem.clone(),
    };
    let new_video_path = target_dir.join(new_file_name);

    if already_in_target_parent && is_same_path_for_fs(&current_video_path, &new_video_path) {
        return Ok(None);
    }

    let actual_poster_source = resolve_asset_source(video_path_obj, poster, "poster");
    let actual_thumb_source = resolve_asset_source(video_path_obj, thumb, "thumb");
    let actual_fanart_source = resolve_asset_source(video_path_obj, fanart, "fanart");

    let poster_source = actual_poster_source
        .as_ref()
        .map(|path| remap_path_after_dir_rename(path, old_dir, &target_dir));
    let thumb_source = actual_thumb_source
        .as_ref()
        .map(|path| remap_path_after_dir_rename(path, old_dir, &target_dir));
    let fanart_source = actual_fanart_source
        .as_ref()
        .map(|path| remap_path_after_dir_rename(path, old_dir, &target_dir));

    let poster_target = poster_source
        .as_ref()
        .and_then(|source| build_artwork_target_path(source, &target_dir, &new_stem, "poster"));
    let thumb_target = thumb_source
        .as_ref()
        .and_then(|source| build_artwork_target_path(source, &target_dir, &new_stem, "thumb"));
    let fanart_target = fanart_source
        .as_ref()
        .and_then(|source| build_artwork_target_path(source, &target_dir, &new_stem, "fanart"));

    let mut operations = Vec::new();
    if rename_parent_dir && !is_same_path_for_fs(old_dir, &target_dir) {
        operations.push(PendingRenameOperation {
            source: old_dir.to_path_buf(),
            target: target_dir.clone(),
            is_dir: true,
        });
    }

    if !is_same_path_for_fs(&current_video_path, &new_video_path) {
        operations.push(PendingRenameOperation {
            source: current_video_path.clone(),
            target: new_video_path.clone(),
            is_dir: false,
        });
    }

    let actual_nfo_source = video_path_obj.with_extension("nfo");
    let current_nfo = if rename_parent_dir {
        remap_path_after_dir_rename(&actual_nfo_source, old_dir, &target_dir)
    } else {
        actual_nfo_source.clone()
    };
    let new_nfo = new_video_path.with_extension("nfo");
    if actual_nfo_source.exists() && !is_same_path_for_fs(&current_nfo, &new_nfo) {
        operations.push(PendingRenameOperation {
            source: current_nfo,
            target: new_nfo,
            is_dir: false,
        });
    }

    queue_optional_file_rename(&mut operations, poster_source.clone(), poster_target.clone());
    queue_optional_file_rename(&mut operations, thumb_source.clone(), thumb_target.clone());
    queue_optional_file_rename(&mut operations, fanart_source.clone(), fanart_target.clone());

    if !rename_parent_dir {
        let extrafanart_source = old_dir.join(EXTRAFANART_DIR_NAME);
        let extrafanart_target = target_dir.join(EXTRAFANART_DIR_NAME);
        if extrafanart_source.exists() && extrafanart_source.is_dir()
            && !is_same_path_for_fs(&extrafanart_source, &extrafanart_target)
        {
            operations.push(PendingRenameOperation {
                source: extrafanart_source,
                target: extrafanart_target,
                is_dir: true,
            });
        }
    }

    queue_matching_subtitle_renames(
        &mut operations,
        video_path_obj,
        old_dir,
        &target_dir,
        &new_stem,
        rename_parent_dir,
    );

    if operations.is_empty() {
        return Ok(None);
    }

    execute_rename_operations(&operations)?;

    Ok(Some(RelocatedVideoAssets {
        original_video_path: video_path.to_string(),
        video_path: new_video_path.to_string_lossy().to_string(),
        dir_path: target_dir.to_string_lossy().to_string(),
        // 未随视频搬动的图（如独立目录里的图）保留其原路径，避免被写库清空
        poster: poster_target
            .or(poster_source)
            .map(|path| path.to_string_lossy().to_string())
            .or_else(|| poster.map(|p| p.to_string())),
        thumb: thumb_target
            .or(thumb_source)
            .map(|path| path.to_string_lossy().to_string())
            .or_else(|| thumb.map(|p| p.to_string())),
        fanart: fanart_target
            .or(fanart_source)
            .map(|path| path.to_string_lossy().to_string())
            .or_else(|| fanart.map(|p| p.to_string())),
        moved_siblings: Vec::new(),
    }))
}

/// 分段影片的组目录名：去分段后缀的基名，保留文件名原大小写
/// （[`parse_stack_part`] 产出的是大写归一基名，不宜直接拿来建目录）。非分段影片返回 `None`。
fn stack_group_dir_name(file_stem: &str) -> Option<String> {
    let (base, _) = crate::utils::designation_recognizer::parse_stack_part(file_stem)?;
    Some(
        file_stem
            .get(..base.len())
            .filter(|head| head.eq_ignore_ascii_case(&base))
            .map(str::to_string)
            .unwrap_or(base),
    )
}

/// 把同目录里属于同一分段组的其它段（连同各自的 NFO / 图集 / 字幕）搬进组目录，
/// 返回搬动的段（原路径 → 新路径）。单个段搬不动只记日志跳过，不中断主流程。
fn move_stack_siblings(
    parent_dir: &Path,
    target_dir: &Path,
    stack_base: &str,
    current_video: &Path,
) -> Vec<MovedStackSibling> {
    use crate::scanner::file_scanner::is_video_file;
    use crate::utils::designation_recognizer::parse_stack_part;

    let Ok(entries) = fs::read_dir(parent_dir) else {
        return Vec::new();
    };
    let mut moved = Vec::new();
    for entry in entries.flatten() {
        let candidate = entry.path();
        if candidate == current_video || !candidate.is_file() || !is_video_file(&candidate) {
            continue;
        }
        let same_group = candidate
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(parse_stack_part)
            .is_some_and(|(base, _)| base.eq_ignore_ascii_case(stack_base));
        if !same_group {
            continue;
        }
        let Some(file_name) = candidate.file_name() else {
            continue;
        };
        let target = target_dir.join(file_name);
        if target.exists() {
            log::warn!(
                "[media_assets] event=move_stack_sibling_skipped_target_exists source={} target={}",
                candidate.display(),
                target.display()
            );
            continue;
        }
        if let Err(error) = move_file(&candidate, &target) {
            log::error!(
                "[media_assets] event=move_stack_sibling_failed source={} target={} error={}",
                candidate.display(),
                target.display(),
                error
            );
            continue;
        }

        let nfo = candidate.with_extension("nfo");
        if nfo.exists() {
            let new_nfo = target.with_extension("nfo");
            if let Err(error) = move_file(&nfo, &new_nfo) {
                log::error!(
                    "[media_assets] event=move_nfo_failed source={} target={} error={}",
                    nfo.display(),
                    new_nfo.display(),
                    error
                );
            }
        }
        for suffix in ["poster", "thumb", "fanart"] {
            move_optional_asset(
                find_sibling_artwork(&candidate, suffix).map(PathBuf::from),
                target_dir,
                suffix,
            );
        }
        move_matching_subtitle_files(&candidate, target_dir);

        moved.push(MovedStackSibling {
            original_video_path: candidate.to_string_lossy().to_string(),
            video_path: target.to_string_lossy().to_string(),
        });
    }
    moved
}

/// 把视频归入同名目录（`ABC-123.mp4` → `ABC-123/ABC-123.mp4`），NFO / 图集 / 字幕随之搬入。
///
/// 分段影片（`FC2-780185-1.mp4`）改归入以组基名命名的目录（`FC2-780185/`），并把同目录里的
/// 同组其它段一并搬入（见 [`move_stack_siblings`]）：只搬当前段会把合集拆散——
/// `FC2-780185-1/` 里只有第 1 段，第 2、3 段留在原地。
pub fn ensure_video_in_named_parent_dir(
    video_path: &str,
    poster: Option<&str>,
    thumb: Option<&str>,
    fanart: Option<&str>,
) -> Result<Option<RelocatedVideoAssets>, String> {
    let video_path_obj = Path::new(video_path);
    // 已在同名目录，或分段影片已与同组其它段同在一个组目录：不搬动
    if has_same_named_parent_dir(video_path_obj) || is_stack_part_in_grouped_dir(video_path_obj) {
        return Ok(None);
    }

    let parent_dir = video_path_obj.parent().ok_or("无效的视频路径")?;
    let file_stem = video_path_obj
        .file_stem()
        .ok_or("无效的视频文件名")?
        .to_string_lossy()
        .to_string();
    let file_name = video_path_obj.file_name().ok_or("无效的视频文件名")?;

    let stack_base = stack_group_dir_name(&file_stem);
    let target_dir = parent_dir.join(stack_base.as_deref().unwrap_or(&file_stem));
    fs::create_dir_all(&target_dir).map_err(|e| format!("创建同名目录失败: {}", e))?;

    let new_video_path = target_dir.join(file_name);
    if new_video_path.exists() {
        return Err(format!(
            "目标目录已存在同名视频文件: {}",
            new_video_path.display()
        ));
    }

    move_file(video_path_obj, &new_video_path).map_err(|e| format!("移动视频文件失败: {}", e))?;

    let current_nfo = video_path_obj.with_extension("nfo");
    if current_nfo.exists() {
        let new_nfo = new_video_path.with_extension("nfo");
        if let Err(error) = move_file(&current_nfo, &new_nfo) {
            log::error!(
                "[media_assets] event=move_nfo_failed source={} target={} error={}",
                current_nfo.display(),
                new_nfo.display(),
                error
            );
        }
    }

    let new_poster = move_optional_asset(
        resolve_asset_source(video_path_obj, poster, "poster"),
        &target_dir,
        "poster",
    );
    let new_thumb = move_optional_asset(
        resolve_asset_source(video_path_obj, thumb, "thumb"),
        &target_dir,
        "thumb",
    );
    let new_fanart = move_optional_asset(
        resolve_asset_source(video_path_obj, fanart, "fanart"),
        &target_dir,
        "fanart",
    );

    let extrafanart_dir = parent_dir.join(EXTRAFANART_DIR_NAME);
    if extrafanart_dir.exists() && extrafanart_dir.is_dir() {
        let target_extrafanart_dir = target_dir.join(EXTRAFANART_DIR_NAME);
        if let Err(error) = move_dir(&extrafanart_dir, &target_extrafanart_dir) {
            log::error!(
                "[media_assets] event=move_extrafanart_dir_failed source={} target={} error={}",
                extrafanart_dir.display(),
                target_extrafanart_dir.display(),
                error
            );
        }
    }

    move_matching_subtitle_files(video_path_obj, &target_dir);

    let moved_siblings = match &stack_base {
        Some(base) => move_stack_siblings(parent_dir, &target_dir, base, video_path_obj),
        None => Vec::new(),
    };

    Ok(Some(RelocatedVideoAssets {
        original_video_path: video_path.to_string(),
        video_path: new_video_path.to_string_lossy().to_string(),
        dir_path: target_dir.to_string_lossy().to_string(),
        poster: new_poster,
        thumb: new_thumb,
        fanart: new_fanart,
        moved_siblings,
    }))
}

/// 在指定资产目录下定位 extrafanart 子目录
pub fn extrafanart_dir_in(asset_dir: &Path) -> PathBuf {
    asset_dir.join(EXTRAFANART_DIR_NAME)
}

pub fn extrafanart_dir_for_video(video_path: &Path) -> Result<PathBuf, String> {
    let parent_dir = video_path.parent().ok_or("无效的视频路径")?;
    Ok(extrafanart_dir_in(parent_dir))
}

pub fn find_sibling_artwork(video_path: &Path, suffix: &str) -> Option<String> {
    let parent_dir = video_path.parent()?;
    let file_stem = video_path.file_stem()?.to_string_lossy();

    ["jpg", "jpeg", "png", "webp"]
        .iter()
        .map(|ext| parent_dir.join(format!("{}-{}.{}", file_stem, suffix, ext)))
        .find(|path| path.exists() && path.is_file())
        .map(|path| path.to_string_lossy().to_string())
}

fn is_supported_image_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png" | "webp"))
        .unwrap_or(false)
}

fn parse_fanart_index(path: &Path) -> Option<usize> {
    let stem = path.file_stem()?.to_str()?;
    let suffix = stem.strip_prefix("fanart")?;
    suffix.parse::<usize>().ok()
}

/// 收集指定资产目录下 extrafanart/fanartN.* 的 (序号, 路径)，按序号升序。
pub fn collect_extrafanart_in(asset_dir: &Path) -> Vec<(usize, String)> {
    let extrafanart_dir = extrafanart_dir_in(asset_dir);
    if !extrafanart_dir.exists() || !extrafanart_dir.is_dir() {
        return Vec::new();
    }

    let mut paths = Vec::new();
    if let Ok(entries) = fs::read_dir(&extrafanart_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || !is_supported_image_file(&path) {
                continue;
            }

            if let Some(index) = parse_fanart_index(&path) {
                paths.push((index, path.to_string_lossy().to_string()));
            }
        }
    }
    paths.sort_by_key(|(index, _)| *index);
    paths
}

pub fn collect_extrafanart_paths(video_path: &Path) -> Vec<(usize, String)> {
    match video_path.parent() {
        Some(parent) => collect_extrafanart_in(parent),
        None => Vec::new(),
    }
}

/// 指定资产目录下的下一个可用 extrafanart 序号（追加预览图，不覆盖已有）。
pub fn next_extrafanart_index_in(asset_dir: &Path) -> usize {
    collect_extrafanart_in(asset_dir)
        .into_iter()
        .map(|(index, _)| index)
        .max()
        .unwrap_or(0)
        + 1
}

pub fn next_extrafanart_index(video_path: &Path) -> usize {
    video_path
        .parent()
        .map(next_extrafanart_index_in)
        .unwrap_or(1)
}

pub async fn sync_extrafanart_from_urls(
    video_path: &str,
    images: Vec<(usize, String)>,
) -> Result<Vec<String>, String> {
    let video_parent = Path::new(video_path).parent().ok_or("无效的视频路径")?;
    sync_extrafanart_to_dir(video_parent, images).await
}

/// 下载预览图到 `<asset_dir>/extrafanart/`，文件名 `fanart{N}.jpg`，有界并发。
///
/// 供独立目录模式将预览图写入 `<root>/<番号 标题>/extrafanart/`。
pub async fn sync_extrafanart_to_dir(
    asset_dir: &Path,
    images: Vec<(usize, String)>,
) -> Result<Vec<String>, String> {
    if images.is_empty() {
        return Ok(Vec::new());
    }

    let extrafanart_dir = extrafanart_dir_in(asset_dir);
    fs::create_dir_all(&extrafanart_dir).map_err(|e| format!("创建 extrafanart 目录失败: {}", e))?;

    let client = std::sync::Arc::new(crate::resource_scrape::fingerprint_client::shared_client()?);
    // 有界并发下载预览图（原先串行逐张，10-20 张时是主要耗时）
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(5));
    let mut handles = Vec::new();

    for (index, url) in images {
        let trimmed = url.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        let save_path = extrafanart_dir.join(format!("fanart{}.jpg", index));
        let client = client.clone();
        let sem = semaphore.clone();
        handles.push(tokio::spawn(async move {
            if save_path.exists() {
                return Some((index, save_path.to_string_lossy().to_string()));
            }
            let _permit = sem.acquire_owned().await.ok()?;
            match crate::download::image::download_image(&client, &trimmed, &save_path).await {
                Ok(path) => {
                    // 过滤太小的预览图（如 125x100 网格缩略图），不算有效图片
                    if crate::media::artwork::is_undersized_preview(&path) {
                        log::info!(
                            "[media_assets] event=extrafanart_too_small index={} url={}",
                            index, trimmed
                        );
                        let _ = fs::remove_file(&save_path);
                        None
                    } else {
                        Some((index, path))
                    }
                }
                Err(e) => {
                    log::error!(
                        "[media_assets] event=download_extrafanart_failed index={} url={} error={}",
                        index, trimmed, e
                    );
                    None
                }
            }
        }));
    }

    // 收集结果并按 index 恢复顺序
    let mut indexed: Vec<(usize, String)> = Vec::new();
    for handle in handles {
        if let Ok(Some(pair)) = handle.await {
            indexed.push(pair);
        }
    }
    indexed.sort_by_key(|(i, _)| *i);
    let saved_paths = indexed.into_iter().map(|(_, p)| p).collect();

    Ok(saved_paths)
}

// ============================================================
// 封面图片
// ============================================================

/// 将截取的视频帧保存为封面图片到指定资产目录（独立目录或视频同级，由调用方解析）。
///
/// # 参数
/// * `dir` - 封面落地目录
/// * `stem` - 文件名 stem（产出 `<stem>-poster/fanart/thumb.*`）
/// * `frame_path` - 截取的帧图片路径
///
/// # 返回
/// * `Ok(String)` - 保存的封面图片路径
/// * `Err(String)` - 保存失败的错误信息
pub fn save_frame_as_cover_assets(
    dir: &Path,
    stem: &str,
    frame_path: &str,
) -> Result<crate::media::artwork::ArtworkResult, String> {
    // 截帧为横版 → fanart + thumb，并右裁出竖版 poster，产出标准图集
    let artwork = crate::media::artwork::produce_artwork_from_local_image(
        dir,
        stem,
        Path::new(frame_path),
    );
    if artwork.fanart.is_none() && artwork.poster.is_none() {
        return Err("保存封面失败".to_string());
    }
    Ok(artwork)
}

/// 将截取的多个视频帧保存到指定资产目录的 extrafanart 子目录（独立目录或视频同级，由调用方解析）。
///
/// # 参数
/// * `asset_dir` - 资产目录（其下 `extrafanart/` 落地预览图）
/// * `frame_paths` - 截取的帧图片路径列表
///
/// # 返回
/// * `Ok(Vec<String>)` - 保存的预览图路径列表
/// * `Err(String)` - 保存失败的错误信息
pub fn save_frames_to_extrafanart(
    asset_dir: &Path,
    frame_paths: &[String],
) -> Result<Vec<String>, String> {
    let extrafanart_dir = extrafanart_dir_in(asset_dir);
    fs::create_dir_all(&extrafanart_dir).map_err(|e| format!("创建 extrafanart 目录失败: {}", e))?;

    let mut next_index = next_extrafanart_index_in(asset_dir);
    let mut thumb_paths = Vec::new();

    for frame_path in frame_paths {
        let thumb_filename = format!("fanart{}.jpg", next_index);
        let thumb_path = extrafanart_dir.join(&thumb_filename);

        fs::copy(frame_path, &thumb_path)
            .map_err(|e| format!("保存预览图 {} 失败: {}", next_index, e))?;

        thumb_paths.push(thumb_path.to_string_lossy().to_string());
        next_index += 1;
    }

    Ok(thumb_paths)
}

// ============================================================
// 视频帧截取 (ffmpeg)
// ============================================================

/// 从视频中随机截取指定数量的帧
///
/// 将视频时长均匀分段，在每段内随机选择时间点，覆盖 0%~100% 范围。
/// 需要系统安装 ffmpeg。
///
/// # 参数
/// * `video_path` - 视频文件路径
/// * `count` - 要截取的帧数量
// 已抽离至 crate::media::ffmpeg

// ============================================================
// 文件回滚
// ============================================================

/// 回滚文件操作，删除已创建的文件
///
/// 当数据库操作失败时调用此函数，以确保文件系统和数据库之间的数据一致性
#[allow(dead_code)]
pub fn rollback_files(
    nfo_path: Option<&std::path::PathBuf>,
    cover_path: Option<&str>,
    thumbs_dir: Option<&std::path::PathBuf>,
) {
    if let Some(nfo) = nfo_path {
        if nfo.exists() {
            match fs::remove_file(nfo) {
                Ok(_) => log::info!("[media_assets] event=rollback_nfo_deleted path={}", nfo.display()),
                Err(e) => log::error!("[media_assets] event=rollback_nfo_delete_failed path={} error={}", nfo.display(), e),
            }
        }
    }

    if let Some(cover) = cover_path {
        if !cover.trim().is_empty() {
            let cover_path_obj = Path::new(cover);
            if cover_path_obj.exists() {
                match fs::remove_file(cover_path_obj) {
                    Ok(_) => log::info!("[media_assets] event=rollback_cover_deleted path={}", cover),
                    Err(e) => log::error!("[media_assets] event=rollback_cover_delete_failed path={} error={}", cover, e),
                }
            }
        }
    }

    if let Some(thumbs) = thumbs_dir {
        if thumbs.exists() {
            match fs::remove_dir_all(thumbs) {
                Ok(_) => log::info!("[media_assets] event=rollback_thumbs_deleted path={}", thumbs.display()),
                Err(e) => log::error!("[media_assets] event=rollback_thumbs_delete_failed path={} error={}", thumbs.display(), e),
            }
        }
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    #[test]
    fn stack_part_in_grouped_dir_is_not_relocated() {
        // 第 2 段已与第 1 段同在以段名 / 基名命名的组目录：视为已归位
        assert!(is_stack_part_in_grouped_dir(Path::new("/lib/05/SIVR-015-1/SIVR-015-2.mp4")));
        assert!(is_stack_part_in_grouped_dir(Path::new("/lib/05/SIVR-015/SIVR-015-2.mp4")));
        assert!(is_stack_part_in_grouped_dir(Path::new("/lib/03/STARS-818-01/STARS-818-02.mp4")));
        // 平铺在普通目录 / 别的番号目录：仍按原逻辑归入同名目录
        assert!(!is_stack_part_in_grouped_dir(Path::new("/lib/05/SIVR-015-2.mp4")));
        assert!(!is_stack_part_in_grouped_dir(Path::new("/lib/05/SIVR-016-1/SIVR-015-2.mp4")));
        // 非分段影片不受影响
        assert!(!is_stack_part_in_grouped_dir(Path::new("/lib/05/SSIS-001/SSIS-002.mp4")));
    }

    #[test]
    fn stack_parts_relocate_together_into_group_dir() {
        // 用户实际场景：三段平铺在普通目录，刮削保存第 1 段时三段应一起归入组目录，不能只搬第 1 段
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let root = dir.path();
        for name in [
            "FC2-780185-1.mp4",
            "FC2-780185-2.mp4",
            "FC2-780185-3.mp4",
            "FC2-780185-2.zh.srt",
            "FC2-780185-3-poster.jpg",
            "OTHER-001.mp4",
        ] {
            fs::write(root.join(name), b"x").expect("写入测试文件失败");
        }

        let first = root.join("FC2-780185-1.mp4");
        let relocated = ensure_video_in_named_parent_dir(first.to_str().unwrap(), None, None, None)
            .expect("归入组目录失败")
            .expect("应当发生搬动");

        // 组目录以基名命名（保留原大小写），当前段搬入
        let group = root.join("FC2-780185");
        assert_eq!(Path::new(&relocated.dir_path), group);
        assert_eq!(Path::new(&relocated.video_path), group.join("FC2-780185-1.mp4"));
        // 同组其它段及其字幕 / 图集随之搬入，原位置不再有
        assert!(group.join("FC2-780185-2.mp4").exists());
        assert!(group.join("FC2-780185-3.mp4").exists());
        assert!(group.join("FC2-780185-2.zh.srt").exists());
        assert!(group.join("FC2-780185-3-poster.jpg").exists());
        assert!(!root.join("FC2-780185-2.mp4").exists());
        assert!(!root.join("FC2-780185-3-poster.jpg").exists());
        // 无关文件不动
        assert!(root.join("OTHER-001.mp4").exists());
        // 回报搬动的段，供调用方同步写库
        let mut moved: Vec<String> = relocated
            .moved_siblings
            .iter()
            .map(|s| Path::new(&s.video_path).file_name().unwrap().to_string_lossy().to_string())
            .collect();
        moved.sort();
        assert_eq!(moved, vec!["FC2-780185-2.mp4", "FC2-780185-3.mp4"]);
        assert!(relocated
            .moved_siblings
            .iter()
            .all(|s| Path::new(&s.original_video_path).parent() == Some(root)));

        // 已在组目录：再次调用不再搬动
        assert!(ensure_video_in_named_parent_dir(&relocated.video_path, None, None, None)
            .unwrap()
            .is_none());
    }

    #[test]
    fn single_video_still_relocates_into_named_dir() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let root = dir.path();
        fs::write(root.join("SSIS-001.mp4"), b"x").expect("写入测试文件失败");
        let relocated =
            ensure_video_in_named_parent_dir(root.join("SSIS-001.mp4").to_str().unwrap(), None, None, None)
                .unwrap()
                .unwrap();
        assert_eq!(Path::new(&relocated.video_path), root.join("SSIS-001").join("SSIS-001.mp4"));
        assert!(relocated.moved_siblings.is_empty());
    }

    #[test]
    fn rename_with_title_skips_stack_part() {
        // 分段影片按标题改名会与同组其它段脱节，应跳过并保持原文件不动
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let path = dir.path().join("FC2-780185-1.mp4");
        fs::write(&path, b"x").expect("写入测试文件失败");
        let result = rename_video_assets_with_title(path.to_str().unwrap(), "FC2-780185", None, None, None)
            .expect("不应报错");
        assert!(result.is_none());
        assert!(path.exists());
    }

    #[test]
    fn test_rollback_files_deletes_nfo() {
        let temp_dir = std::env::temp_dir();
        let nfo_path = temp_dir.join("test_video.nfo");

        let mut file = fs::File::create(&nfo_path).unwrap();
        file.write_all(b"test nfo content").unwrap();
        drop(file);

        assert!(nfo_path.exists());
        rollback_files(Some(&nfo_path), None, None);
        assert!(!nfo_path.exists());
    }

    #[test]
    fn test_rollback_files_deletes_cover() {
        let temp_dir = std::env::temp_dir();
        let cover_path = temp_dir.join("test_video-poster.jpg");

        let mut file = fs::File::create(&cover_path).unwrap();
        file.write_all(b"fake image data").unwrap();
        drop(file);

        assert!(cover_path.exists());
        let cover_str = cover_path.to_string_lossy().to_string();
        rollback_files(None, Some(&cover_str), None);
        assert!(!cover_path.exists());
    }

    #[test]
    fn test_rollback_files_deletes_thumbs_directory() {
        let temp_dir = std::env::temp_dir();
        let thumbs_dir = temp_dir.join("test_thumbs");
        fs::create_dir_all(&thumbs_dir).unwrap();

        for i in 1..=3 {
            let thumb_path = thumbs_dir.join(format!("thumb_{:03}.jpg", i));
            let mut file = fs::File::create(&thumb_path).unwrap();
            file.write_all(b"fake thumb data").unwrap();
        }

        assert!(thumbs_dir.exists());
        assert_eq!(fs::read_dir(&thumbs_dir).unwrap().count(), 3);

        rollback_files(None, None, Some(&thumbs_dir));
        assert!(!thumbs_dir.exists());
    }

    #[test]
    fn test_rollback_files_deletes_all() {
        let temp_dir = std::env::temp_dir();
        let nfo_path = temp_dir.join("test_all.nfo");
        let cover_path = temp_dir.join("test_all-poster.jpg");
        let thumbs_dir = temp_dir.join("test_all_thumbs");

        fs::File::create(&nfo_path)
            .unwrap()
            .write_all(b"nfo")
            .unwrap();
        fs::File::create(&cover_path)
            .unwrap()
            .write_all(b"cover")
            .unwrap();
        fs::create_dir_all(&thumbs_dir).unwrap();
        fs::File::create(thumbs_dir.join("thumb_001.jpg"))
            .unwrap()
            .write_all(b"thumb")
            .unwrap();

        assert!(nfo_path.exists());
        assert!(cover_path.exists());
        assert!(thumbs_dir.exists());

        let cover_str = cover_path.to_string_lossy().to_string();
        rollback_files(Some(&nfo_path), Some(&cover_str), Some(&thumbs_dir));

        assert!(!nfo_path.exists());
        assert!(!cover_path.exists());
        assert!(!thumbs_dir.exists());
    }

    #[test]
    fn test_rollback_files_handles_nonexistent_files() {
        let temp_dir = std::env::temp_dir();
        let nonexistent_nfo = temp_dir.join("nonexistent.nfo");
        let nonexistent_cover = temp_dir.join("nonexistent-poster.jpg");
        let nonexistent_thumbs = temp_dir.join("nonexistent_thumbs");

        assert!(!nonexistent_nfo.exists());
        assert!(!nonexistent_cover.exists());
        assert!(!nonexistent_thumbs.exists());

        let cover_str = nonexistent_cover.to_string_lossy().to_string();
        rollback_files(
            Some(&nonexistent_nfo),
            Some(&cover_str),
            Some(&nonexistent_thumbs),
        );

        assert!(!nonexistent_nfo.exists());
        assert!(!nonexistent_cover.exists());
        assert!(!nonexistent_thumbs.exists());
    }

}
