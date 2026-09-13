use std::collections::{HashMap, HashSet};
use std::path::Path;
use tauri::AppHandle;
use tauri::State;

use crate::error::{AppError, AppResult};
use crate::utils::designation_recognizer::stack_scope_dir;

use super::service::{
    clear_video_scrape_data, copy_dir_recursive, delete_video_and_files,
    ensure_video_in_own_dir_with_db, move_file,
    update_all_directories_count, AdVideo, VideoUpdateContext, VideoUpdatePayload,
    VideoUpdateResult, build_nfo_metadata_for_update, load_video_relation_names,
    parse_name_list,
};

// ==================== 目录管理 ====================

#[tauri::command]
pub async fn get_directories(db: State<'_, crate::db::Database>) -> AppResult<Vec<serde_json::Value>> {
    let conn = db.get_connection()?;

    tokio::task::spawn_blocking(move || {
        let mut stmt = conn
            .prepare("SELECT id, path, video_count, created_at, updated_at FROM directories ORDER BY created_at DESC")?;
        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let path: String = row.get(1)?;
                let count: i64 = row.get(2)?;
                let created_at: String = row.get(3)?;
                let updated_at: String = row.get(4)?;
                Ok(serde_json::json!({
                    "id": id,
                    "path": path,
                    "videoCount": count,
                    "createdAt": created_at,
                    "updatedAt": updated_at
                }))
            })?;

        let mut dirs = Vec::new();
        for r in rows {
            dirs.push(r?);
        }
        Ok(dirs)
    })
    .await
    .map_err(|e| AppError::TaskJoin(e.to_string()))?
}

#[tauri::command]
pub async fn add_directory(db: State<'_, crate::db::Database>, path: String) -> AppResult<String> {
    use uuid::Uuid;

    if crate::scanner::file_scanner::is_skipped_directory(Path::new(&path)) {
        return Err(AppError::Business("该目录已被系统忽略，不能添加：behind the scenes / backdrops".to_string()));
    }

    let conn = db.get_connection()?;

    tokio::task::spawn_blocking(move || {
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM directories WHERE path = ?",
                [&path],
                |row| row.get(0),
            )?;

        if exists {
            return Err(AppError::Business("目录已存在".to_string()));
        }

        let id = Uuid::new_v4().to_string();

        conn.execute(
            "INSERT INTO directories (id, path, video_count) VALUES (?, ?, 0)",
            rusqlite::params![&id, &path],
        )?;

        Ok(id)
    })
    .await
    .map_err(|e| AppError::TaskJoin(e.to_string()))?
}

#[tauri::command]
pub async fn delete_directory(db: State<'_, crate::db::Database>, id: String) -> AppResult<()> {
    let conn = db.get_connection()?;

    tokio::task::spawn_blocking(move || {
        let path: String = conn
            .query_row("SELECT path FROM directories WHERE id = ?", [&id], |row| {
                row.get(0)
            })?;

        crate::db::Database::delete_videos_in_directory(&conn, &path)?;

        conn.execute("DELETE FROM directories WHERE id = ?", [&id])?;

        Ok(())
    })
    .await
    .map_err(|e| AppError::TaskJoin(e.to_string()))?
}

// ==================== 视频管理 ====================

/// 将同一影片的分段文件折叠为单张代表卡。
///
/// 按 `(分段范围目录, stackKey)` 分组，组员 ≥2 时：取段序号（`partIndex`）最小者为代表，
/// 为其注入 `parts`（各段 videoPath/partIndex/duration/fileSize/title，按序号排序）、
/// `partCount`，并把 `duration` 覆盖为各段总时长；其余段从列表移除。
/// `stackKey` 为空或组员仅 1 的视频原样保留（无 `parts`/`partCount`）。
///
/// 范围目录见 [`stack_scope_dir`]：各段可能被「归入同名目录」分别装进以段名命名的子目录
/// （`SIVR-015-1/SIVR-015-1.mp4` + `SIVR-015-1/SIVR-015-2/SIVR-015-2.mp4`），按直接父目录
/// 分组会把同一影片拆成两张卡。
fn fold_video_stacks(videos: &mut Vec<serde_json::Value>) {
    // 分组：key = (分段范围目录, stackKey) → 组员在 videos 中的下标
    let mut groups: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (idx, v) in videos.iter().enumerate() {
        if let (Some(dir), Some(key)) = (
            v.get("dirPath").and_then(|x| x.as_str()),
            v.get("stackKey").and_then(|x| x.as_str()),
        ) {
            if !key.is_empty() {
                let scope = stack_scope_dir(dir, key);
                groups.entry((scope, key.to_string())).or_default().push(idx);
            }
        }
    }

    let part_index_of = |v: &serde_json::Value| v.get("partIndex").and_then(|x| x.as_i64()).unwrap_or(i64::MAX);

    let mut drop_indices: Vec<usize> = Vec::new();
    // (代表下标, parts 数组, 总时长, 段数)
    let mut rep_updates: Vec<(usize, serde_json::Value, i64, i64)> = Vec::new();

    for (_, mut members) in groups {
        if members.len() < 2 {
            continue;
        }
        // 按 (段序号, videoPath) 排序，确保代表与 parts 顺序稳定
        members.sort_by(|&a, &b| {
            part_index_of(&videos[a]).cmp(&part_index_of(&videos[b])).then_with(|| {
                let va = videos[a].get("videoPath").and_then(|x| x.as_str()).unwrap_or("");
                let vb = videos[b].get("videoPath").and_then(|x| x.as_str()).unwrap_or("");
                va.cmp(vb)
            })
        });

        let mut parts = Vec::with_capacity(members.len());
        let mut total_duration = 0i64;
        for &m in &members {
            total_duration += videos[m].get("duration").and_then(|x| x.as_i64()).unwrap_or(0);
            parts.push(serde_json::json!({
                "videoPath": videos[m].get("videoPath").cloned().unwrap_or(serde_json::Value::Null),
                "partIndex": videos[m].get("partIndex").cloned().unwrap_or(serde_json::Value::Null),
                "duration": videos[m].get("duration").cloned().unwrap_or(serde_json::Value::Null),
                "fileSize": videos[m].get("fileSize").cloned().unwrap_or(serde_json::Value::Null),
                "title": videos[m].get("title").cloned().unwrap_or(serde_json::Value::Null),
            }));
        }

        let count = members.len() as i64;
        drop_indices.extend_from_slice(&members[1..]);
        rep_updates.push((members[0], serde_json::Value::Array(parts), total_duration, count));
    }

    // 先写代表行（此时不再持有 groups 对 videos 的借用）
    for (rep, parts, total_duration, count) in rep_updates {
        if let Some(obj) = videos[rep].as_object_mut() {
            obj.insert("parts".to_string(), parts);
            obj.insert("partCount".to_string(), serde_json::json!(count));
            obj.insert("duration".to_string(), serde_json::json!(total_duration));
        }
    }

    // 移除被折叠的非代表段
    if !drop_indices.is_empty() {
        let drop_set: std::collections::HashSet<usize> = drop_indices.into_iter().collect();
        let mut idx = 0usize;
        videos.retain(|_| {
            let keep = !drop_set.contains(&idx);
            idx += 1;
            keep
        });
    }
}

#[tauri::command]
pub async fn get_videos(
    db: State<'_, crate::db::Database>,
) -> AppResult<Vec<serde_json::Value>> {
    let conn = db.get_connection()?;

    tokio::task::spawn_blocking(move || -> AppResult<Vec<serde_json::Value>> {
        let sql = r#"
            SELECT
                v.id,
                v.title,
                v.video_path,
                v.studio,
                v.premiered,
                v.rating,
                v.duration,
                v.created_at,
                v.scan_status,
                v.director,
                v.local_id,
                v.poster,
                v.thumb,
                v.fanart,
                v.original_title,
                (
                    SELECT GROUP_CONCAT(a.name, ', ')
                    FROM video_actors va
                    JOIN actors a ON va.actor_id = a.id
                    WHERE va.video_id = v.id
                    ORDER BY va.priority
                ) as actors,
                v.resolution,
                v.file_size,
                (
                    SELECT GROUP_CONCAT(t.name, ', ')
                    FROM video_tags vt
                    JOIN tags t ON vt.tag_id = t.id
                    WHERE vt.video_id = v.id
                ) as tags,
                (
                    SELECT GROUP_CONCAT(g.name, ', ')
                    FROM video_genres vg
                    JOIN genres g ON vg.genre_id = g.id
                    WHERE vg.video_id = v.id
                ) as genres,
                v.fast_hash,
                v.cover_width,
                v.cover_height,
                v.is_uncensored,
                v.cover_thumb,
                v.stack_key,
                v.part_index,
                v.has_subtitle,
                v.file_ctime,
                v.file_mtime,
                v.scraped_at
            FROM videos v
        "#;
        // 注意：不在 SQL 里排序，最终顺序在下方按 file_ctime（库列）倒序重排。

        let mut stmt = conn.prepare(sql)?;

        let video_iter = stmt
            .query_map([], |row| {
                let poster: Option<String> = row.get(11)?;
                let thumb: Option<String> = row.get(12)?;
                let cover_thumb: Option<String> = row.get(24)?;
                let file_ctime: Option<i64> = row.get(28)?;
                let file_mtime: Option<i64> = row.get(29)?;

                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "title": row.get::<_, Option<String>>(1)?,
                    "videoPath": row.get::<_, String>(2)?,
                    "dirPath": std::path::Path::new(&row.get::<_, String>(2)?)
                        .parent()
                        .map(|path| path.to_string_lossy().to_string()),
                    "studio": row.get::<_, Option<String>>(3)?,
                    "premiered": row.get::<_, Option<String>>(4)?,
                    "rating": row.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
                    "duration": row.get::<_, Option<i64>>(6)?.unwrap_or(0),
                    "createdAt": row.get::<_, String>(7)?,
                    "scanStatus": row.get::<_, i32>(8)?,
                    "director": row.get::<_, Option<String>>(9)?,
                    "localId": row.get::<_, Option<String>>(10)?,
                    // 图集路径直接来自库列：扫描时已按存在性过滤并维护，列表不再逐张 exists()
                    "poster": poster,
                    "thumb": thumb,
                    "fanart": row.get::<_, Option<String>>(13)?,
                    "originalTitle": row.get::<_, Option<String>>(14)?,
                    "actors": row.get::<_, Option<String>>(15)?,
                    "resolution": row.get::<_, Option<String>>(16)?,
                    "fileSize": row.get::<_, Option<i64>>(17)?,
                    "tags": row.get::<_, Option<String>>(18)?,
                    "genres": row.get::<_, Option<String>>(19)?,
                    "fastHash": row.get::<_, Option<String>>(20)?,
                    "coverWidth": row.get::<_, Option<i64>>(21)?,
                    "coverHeight": row.get::<_, Option<i64>>(22)?,
                    "isUncensored": row.get::<_, Option<i64>>(23)?.unwrap_or(0) != 0,
                    "coverThumb": cover_thumb,
                    // 分段归并键（去后缀基名）与段序号，仅用于下方 (dirPath, stackKey) 折叠
                    "stackKey": row.get::<_, Option<String>>(25)?,
                    "partIndex": row.get::<_, Option<i64>>(26)?,
                    // 字幕标记来自库列（扫描/字幕下载时维护），列表不再实时探测文件系统
                    "hasSubtitle": row.get::<_, Option<i64>>(27)?.unwrap_or(0) != 0,
                    // 文件时间来自库列（扫描时写入；旧库由 backfill_file_ctimes 一次性补齐），
                    // 之前每次加载都逐视频 stat，SMB/USB 库上实测十余秒
                    "fileCreatedAt": file_ctime.and_then(millis_to_rfc3339),
                    "fileModifiedAt": file_mtime.and_then(millis_to_rfc3339),
                    "fileCtimeMillis": file_ctime,
                    // 刮削时间：重新刮削会把新封面写回原路径，前端靠它判断封面内容已变、破图片缓存
                    "scrapedAt": row.get::<_, Option<String>>(30)?,
                }))
            })?;

        let mut videos = Vec::new();
        for video in video_iter {
            videos.push(video?);
        }

        // 仅保留位于「目录管理」内的视频，避免下载到库外的文件污染媒体库
        let managed_prefixes = crate::db::Database::managed_directory_prefixes(&conn)?;
        videos.retain(|video| {
            video
                .get("videoPath")
                .and_then(|p| p.as_str())
                .map(|path| {
                    crate::db::Database::is_path_under_managed_directory(&managed_prefixes, path)
                })
                .unwrap_or(false)
        });

        // 按文件创建时间倒序（缺失者排最后），与之前的实时探测排序口径一致
        videos.sort_by(|left, right| {
            let key = |v: &serde_json::Value| v.get("fileCtimeMillis").and_then(|x| x.as_i64());
            match (key(left), key(right)) {
                (Some(l), Some(r)) => r.cmp(&l),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
        for video in videos.iter_mut() {
            if let Some(obj) = video.as_object_mut() {
                obj.remove("fileCtimeMillis");
            }
        }

        // 分段折叠：同 (dirPath, stackKey) 且组员 ≥2 的文件合并为一张代表卡，
        // 代表取段序号最小者，附 parts 列表与 partCount，duration 汇总为总时长；
        // 其余段从列表移除。单文件或 stackKey 为空者原样保留。
        fold_video_stacks(&mut videos);

        Ok(videos)
    })
    .await
    .map_err(|e| AppError::TaskJoin(e.to_string()))?
}

/// 毫秒时间戳 → RFC3339（前端筛选/排序沿用字符串时间）
fn millis_to_rfc3339(millis: i64) -> Option<String> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(millis).map(|dt| dt.to_rfc3339())
}

/// 获取演员列表（含头像与本地作品数），供「发现」页演员分面显示头像。
/// avatarPath 为本地下载头像、avatarUrl 为远程头像（前端优先本地、回退远程）。
#[tauri::command]
pub async fn get_actors(db: State<'_, crate::db::Database>) -> AppResult<Vec<serde_json::Value>> {
    let conn = db.get_connection()?;

    tokio::task::spawn_blocking(move || -> AppResult<Vec<serde_json::Value>> {
        let mut stmt = conn.prepare(
            "SELECT a.id, a.name, a.avatar_path, a.avatar_url,
                    (SELECT COUNT(*) FROM video_actors va WHERE va.actor_id = a.id) AS video_count
             FROM actors a",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, i64>(0)?,
                "name": row.get::<_, String>(1)?,
                "avatarPath": row.get::<_, Option<String>>(2)?,
                "avatarUrl": row.get::<_, Option<String>>(3)?,
                "videoCount": row.get::<_, i64>(4)?,
            }))
        })?;

        let mut actors = Vec::new();
        for r in rows {
            actors.push(r?);
        }
        Ok(actors)
    })
    .await
    .map_err(|e| AppError::TaskJoin(e.to_string()))?
}

/// 回填存量视频的封面尺寸：扫描有 poster 但缺 cover_width/cover_height 的记录，
/// 仅读图头补算尺寸写回。瀑布流等高画廊布局/虚拟化需要封面比例。
/// 返回成功补算的数量。
/// 回填任务的失败重试间隔：失败项 7 天内不再重试，避免每次启动都对同一批无法处理的文件
/// （AVIF 假 jpg、网络盘写失败）重复跑一遍、拖慢启动。
const BACKFILL_RETRY_AFTER: &str = "-7 days";

#[tauri::command]
pub async fn backfill_cover_dimensions(db: State<'_, crate::db::Database>) -> AppResult<u32> {
    let conn = db.get_connection()?;

    tokio::task::spawn_blocking(move || -> AppResult<u32> {
        let targets: Vec<(String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT id, poster FROM videos
                 WHERE poster IS NOT NULL AND poster <> ''
                   AND (cover_width IS NULL OR cover_height IS NULL)
                   AND (cover_dims_attempted_at IS NULL
                        OR cover_dims_attempted_at < datetime('now', ?1))",
            )?;
            let iter = stmt.query_map([BACKFILL_RETRY_AFTER], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut list = Vec::new();
            for item in iter {
                list.push(item?);
            }
            list
        };

        let mut updated = 0u32;
        for (id, poster) in targets {
            match image::image_dimensions(&poster) {
                Ok((w, h)) if w > 0 && h > 0 => {
                    conn.execute(
                        "UPDATE videos SET cover_width = ?, cover_height = ?, cover_dims_attempted_at = datetime('now') WHERE id = ?",
                        rusqlite::params![w as i64, h as i64, id],
                    )?;
                    updated += 1;
                }
                _ => {
                    conn.execute(
                        "UPDATE videos SET cover_dims_attempted_at = datetime('now') WHERE id = ?",
                        rusqlite::params![id],
                    )?;
                }
            }
        }
        Ok(updated)
    })
    .await
    .map_err(|e| AppError::TaskJoin(e.to_string()))?
}

/// 回填存量视频的网格缩略图：扫描有封面但缺 `cover_thumb` 的记录，从横版大图
/// （fanart → thumb → poster）生成 `<stem>-thumbsm.jpg` 小图并写回。
///
/// 媒体库网格逐张解码全尺寸大图是列表卡顿的主因，缩略图回填后前端优先用它。
/// 返回成功生成的数量。生成失败的记录记下尝试时间，[`BACKFILL_RETRY_AFTER`] 内不再重试。
#[tauri::command]
pub async fn backfill_cover_thumbnails(db: State<'_, crate::db::Database>) -> AppResult<u32> {
    let conn = db.get_connection()?;

    tokio::task::spawn_blocking(move || -> AppResult<u32> {
        let targets: Vec<(String, String, String, Option<String>, Option<String>, Option<String>)> = {
            let mut stmt = conn.prepare(
                "SELECT id, video_path, dir_path, fanart, thumb, poster FROM videos
                 WHERE (cover_thumb IS NULL OR cover_thumb = '')
                   AND (
                        (fanart IS NOT NULL AND fanart <> '')
                     OR (thumb IS NOT NULL AND thumb <> '')
                     OR (poster IS NOT NULL AND poster <> '')
                   )
                   AND (cover_thumb_attempted_at IS NULL
                        OR cover_thumb_attempted_at < datetime('now', ?1))",
            )?;
            let iter = stmt.query_map([BACKFILL_RETRY_AFTER], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })?;
            let mut list = Vec::new();
            for item in iter {
                list.push(item?);
            }
            list
        };

        let mut updated = 0u32;
        for (id, video_path, dir_path, fanart, thumb, poster) in targets {
            // 与前端选图一致：横版优先（fanart → thumb → poster）
            let source = fanart
                .as_deref()
                .filter(|p| !p.trim().is_empty())
                .or_else(|| thumb.as_deref().filter(|p| !p.trim().is_empty()))
                .or_else(|| poster.as_deref().filter(|p| !p.trim().is_empty()));
            let Some(source) = source else { continue };

            let Some(stem) = std::path::Path::new(&video_path)
                .file_stem()
                .and_then(|name| name.to_str())
            else {
                continue;
            };

            match crate::media::artwork::generate_cover_thumbnail(
                std::path::Path::new(source),
                std::path::Path::new(&dir_path),
                stem,
            ) {
                Some(thumb_path) => {
                    conn.execute(
                        "UPDATE videos SET cover_thumb = ?, cover_thumb_attempted_at = datetime('now') WHERE id = ?",
                        rusqlite::params![thumb_path, id],
                    )?;
                    updated += 1;
                }
                None => {
                    conn.execute(
                        "UPDATE videos SET cover_thumb_attempted_at = datetime('now') WHERE id = ?",
                        rusqlite::params![id],
                    )?;
                }
            }
        }
        Ok(updated)
    })
    .await
    .map_err(|e| AppError::TaskJoin(e.to_string()))?
}

/// 一次性回填旧库的「是否有字幕」列：只处理 `has_subtitle IS NULL` 的记录，按目录分组、
/// 每目录只 read_dir 一次。之后由扫描/字幕下载维护，列表不再实时探测。
/// 返回本次回填的记录数。
#[tauri::command]
pub async fn backfill_subtitle_flags(
    app: AppHandle,
    db: State<'_, crate::db::Database>,
) -> AppResult<u32> {
    let settings = crate::settings::get_settings(app.clone()).await.unwrap_or_default();
    let cfg = crate::media::storage::MetadataStorageConfig::from_settings(&settings);
    let conn = db.get_connection()?;

    tokio::task::spawn_blocking(move || -> AppResult<u32> {
        let targets: Vec<(String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT video_path, COALESCE(local_id, '') FROM videos WHERE has_subtitle IS NULL",
            )?;
            let iter = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
            let mut list = Vec::new();
            for item in iter {
                list.push(item?);
            }
            list
        };
        if targets.is_empty() {
            return Ok(0);
        }

        // 按资产目录分组，同目录多个视频共用一次 read_dir
        let mut by_dir: HashMap<std::path::PathBuf, Vec<(String, String)>> = HashMap::new();
        for (video_path, local_id) in targets {
            let (dir, stem) =
                crate::media::storage::resolve_existing_asset_dir(&video_path, &local_id, &cfg);
            by_dir.entry(dir).or_default().push((video_path, stem));
        }

        let tx = conn.unchecked_transaction()?;
        let mut updated = 0u32;
        for (dir, items) in by_dir {
            let stems = std::fs::read_dir(&dir)
                .map(|entries| {
                    let entries: Vec<std::fs::DirEntry> = entries.flatten().collect();
                    crate::media::assets::collect_subtitle_stems(&entries)
                })
                .unwrap_or_default();
            for (video_path, stem) in items {
                let has = crate::media::assets::stem_has_matching_subtitle(&stems, &stem);
                crate::db::Database::set_video_has_subtitle(&tx, &video_path, has)?;
                updated += 1;
            }
        }
        tx.commit()?;
        Ok(updated)
    })
    .await
    .map_err(|e| AppError::TaskJoin(e.to_string()))?
}

/// 一次性回填旧库的文件创建时间列：只处理 `file_ctime IS NULL` 的记录，并发 stat 后写库。
/// 之后由扫描维护，列表按库列排序、不再实时探测。返回本次回填的记录数。
#[tauri::command]
pub async fn backfill_file_ctimes(db: State<'_, crate::db::Database>) -> AppResult<u32> {
    let conn = db.get_connection()?;
    let targets: Vec<String> = tokio::task::spawn_blocking(move || -> AppResult<Vec<String>> {
        let mut stmt = conn.prepare("SELECT video_path FROM videos WHERE file_ctime IS NULL")?;
        let iter = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut list = Vec::new();
        for item in iter {
            list.push(item?);
        }
        Ok(list)
    })
    .await
    .map_err(|e| AppError::TaskJoin(e.to_string()))??;
    if targets.is_empty() {
        return Ok(0);
    }

    // 文件系统探测是延迟型负载（网络盘/USB 盘尤甚），并发度按核数放大：实测并发 16 与 64 相差约 3 倍
    let max_concurrency = std::thread::available_parallelism()
        .map(|p| p.get() * 4)
        .unwrap_or(16)
        .clamp(8, 64);
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(max_concurrency));
    let mut tasks = tokio::task::JoinSet::new();
    for video_path in targets {
        let semaphore = std::sync::Arc::clone(&semaphore);
        tasks.spawn(async move {
            let _permit = semaphore.acquire_owned().await.ok();
            tokio::task::spawn_blocking(move || {
                let ctime = std::fs::metadata(&video_path).ok().and_then(|m| {
                    let t = m.created().ok().or_else(|| m.modified().ok())?;
                    i64::try_from(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis()).ok()
                });
                (video_path, ctime)
            })
            .await
            .ok()
        });
    }
    let mut results: Vec<(String, i64)> = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        if let Ok(Some((path, Some(ctime)))) = joined {
            results.push((path, ctime));
        }
    }
    if results.is_empty() {
        return Ok(0);
    }

    let conn = db.get_connection()?;
    tokio::task::spawn_blocking(move || -> AppResult<u32> {
        let tx = conn.unchecked_transaction()?;
        for (path, ctime) in &results {
            crate::db::Database::set_video_file_ctime(&tx, path, Some(*ctime))?;
        }
        tx.commit()?;
        Ok(results.len() as u32)
    })
    .await
    .map_err(|e| AppError::TaskJoin(e.to_string()))?
}

#[tauri::command]

pub async fn get_duplicate_videos(db: State<'_, crate::db::Database>) -> AppResult<Vec<serde_json::Value>> {
    let conn = db.get_connection()?;

    tokio::task::spawn_blocking(move || {
        // 同番号重复：把同一分段组（分段范围目录 + stack_key，与列表 fold_video_stacks 口径一致）
        // 折成 1 份再计数，避免把同一影片的多个分段（共享番号）误报为可删重复。
        // 范围目录需按目录名回溯（见 stack_scope_dir），SQL 做不了，先在此算出重复番号集合。
        let dup_local_ids: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT local_id, video_path, stack_key FROM videos WHERE local_id IS NOT NULL AND local_id != ''",
            )?;
            let mut copies: HashMap<String, HashSet<(String, String)>> = HashMap::new();
            for row in stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })? {
                let (local_id, video_path, stack_key) = row?;
                let dir = std::path::Path::new(&video_path)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                let copy = match stack_key.filter(|k| !k.is_empty()) {
                    Some(key) => (stack_scope_dir(&dir, &key), key),
                    None => (dir, video_path),
                };
                copies.entry(local_id).or_default().insert(copy);
            }
            copies
                .into_iter()
                .filter(|(_, copies)| copies.len() > 1)
                .map(|(local_id, _)| local_id)
                .collect()
        };

        // 跨目录查找 fast_hash 重复或 local_id（番号）重复的视频
        let placeholders = vec!["?"; dup_local_ids.len()].join(",");
        let sql = format!(
            r#"
            SELECT
                v.id,
                v.title,
                v.video_path,
                v.dir_path,
                v.local_id,
                v.resolution,
                v.file_size,
                v.fast_hash,
                v.scan_status,
                v.duration
            FROM videos v
            WHERE (v.fast_hash IS NOT NULL AND v.fast_hash != '' AND v.fast_hash IN (
                SELECT fast_hash FROM videos WHERE fast_hash IS NOT NULL AND fast_hash != '' GROUP BY fast_hash HAVING COUNT(*) > 1
            ))
            OR v.local_id IN ({placeholders})
            ORDER BY v.local_id, v.fast_hash, v.created_at DESC
        "#
        );

        let mut stmt = conn.prepare(&sql)?;
        let video_iter = stmt
            .query_map(rusqlite::params_from_iter(dup_local_ids.iter()), |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "title": row.get::<_, Option<String>>(1)?,
                    "videoPath": row.get::<_, String>(2)?,
                    "dirPath": row.get::<_, Option<String>>(3)?,
                    "localId": row.get::<_, Option<String>>(4)?,
                    "resolution": row.get::<_, Option<String>>(5)?,
                    "fileSize": row.get::<_, Option<i64>>(6)?,
                    "fastHash": row.get::<_, Option<String>>(7)?,
                    "scanStatus": row.get::<_, i32>(8)?,
                    "duration": row.get::<_, Option<i32>>(9)?,
                }))
            })?;

        let mut videos = Vec::new();
        for video in video_iter {
            videos.push(video?);
        }

        Ok(videos)
    })
    .await
    .map_err(|e| AppError::TaskJoin(e.to_string()))?
}

#[tauri::command]
pub async fn delete_video_db(db: State<'_, crate::db::Database>, id: String) -> AppResult<()> {
    let conn = db.get_connection()?;

    tokio::task::spawn_blocking(move || {
        conn.execute("DELETE FROM videos WHERE id = ?", [id])?;
        Ok(())
    })
    .await
    .map_err(|e| AppError::TaskJoin(e.to_string()))?
}

#[tauri::command]
pub async fn delete_video_file(
    db: State<'_, crate::db::Database>,
    id: String,
    delete_scrape_data_only: Option<bool>,
) -> AppResult<()> {
    let conn = db.get_connection()?;

    tokio::task::spawn_blocking(move || {
        if delete_scrape_data_only.unwrap_or(false) {
            clear_video_scrape_data(&conn, &id).map_err(|e| AppError::Business(e))?;
        } else {
            delete_video_and_files(&conn, &id).map_err(|e| AppError::Business(e))?;
        }
        let _ = update_all_directories_count(&conn);
        Ok(())
    })
    .await
    .map_err(|e| AppError::TaskJoin(e.to_string()))?
}

#[tauri::command]

pub async fn move_video_file(app: AppHandle, db: State<'_, crate::db::Database>, id: String, target_dir: String) -> AppResult<()> {
    use std::fs;
    use std::path::Path;

    let conn = db.get_connection()?;
    let app_clone = app.clone();
    // 独立目录模式配置（用于移动后同步 .strm）
    let storage_cfg = crate::media::storage::MetadataStorageConfig::from_settings(
        &crate::settings::get_settings(app.clone()).await.unwrap_or_default(),
    );

    tokio::task::spawn_blocking(move || {
        let _app = app_clone;

        // 校验目标路径在已注册的扫描目录范围内
        super::service::validate_path_within_managed_dirs(&conn, Path::new(&target_dir))?;

        // 查询视频路径、同级图路径与番号
        let (current_path, poster, thumb, fanart, local_id): (String, Option<String>, Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT video_path, poster, thumb, fanart, local_id FROM videos WHERE id = ?",
                [&id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )?;

        let current_path_obj = Path::new(&current_path);
        if !current_path_obj.exists() {
            return Err(AppError::Business("源视频文件不存在".to_string()));
        }

        let file_name = current_path_obj.file_name().ok_or_else(|| AppError::Business("无效的文件名".to_string()))?;
        let new_path_obj = Path::new(&target_dir).join(file_name);

        if new_path_obj.exists() {
            return Err(AppError::Business("目标目录已存在同名文件".to_string()));
        }

        // 1. 移动视频文件
        move_file(current_path_obj, &new_path_obj).map_err(|e| AppError::Business(format!("移动视频失败: {}", e)))?;

        // 2. 移动 NFO 文件
        let current_nfo = current_path_obj.with_extension("nfo");
        if current_nfo.exists() {
            let new_nfo = new_path_obj.with_extension("nfo");
            let _ = move_file(&current_nfo, &new_nfo);
        }

        // 3. 移动同级图片资源
        // 仅搬动与视频同级的图；独立元数据目录里的图留在原地（库内路径不变）
        let move_artwork = |path_opt: Option<String>, label: &str| -> AppResult<Option<String>> {
            if let Some(path) = path_opt {
                let source = Path::new(&path);
                if source.exists() && source.parent() == current_path_obj.parent() {
                    let file_name = source.file_name().ok_or_else(|| AppError::Business(format!("无效的{}文件名", label)))?;
                    let target = Path::new(&target_dir).join(file_name);
                    move_file(source, &target).map_err(|e| AppError::Business(format!("移动{}失败: {}", label, e)))?;
                    return Ok(Some(target.to_string_lossy().to_string()));
                }
            }
            Ok(None)
        };
        let new_poster = move_artwork(poster.clone(), "poster")?;
        let new_thumb = move_artwork(thumb.clone(), "thumb")?;
        let new_fanart = move_artwork(fanart.clone(), "fanart")?;

        // 4. 移动 extrafanart 目录
        let old_parent = current_path_obj.parent().ok_or_else(|| AppError::Business("无效的源路径".to_string()))?;
        let extrafanart_dir = old_parent.join("extrafanart");
        if extrafanart_dir.exists() && extrafanart_dir.is_dir() {
            let target_extrafanart_dir = Path::new(&target_dir).join("extrafanart");
            copy_dir_recursive(&extrafanart_dir, &target_extrafanart_dir)
                .map_err(|e| AppError::Business(format!("移动 extrafanart 目录失败: {}", e)))?;
            let _ = fs::remove_dir_all(&extrafanart_dir);
        }

        // 5. 更新数据库
        let new_path_str = new_path_obj.to_string_lossy().to_string();
        conn.execute(
            "UPDATE videos SET video_path = ?, dir_path = ?, poster = ?, thumb = ?, fanart = ?, updated_at = datetime('now') WHERE id = ?",
            rusqlite::params![
                new_path_str,
                target_dir,
                new_poster.or(poster),
                new_thumb.or(thumb),
                new_fanart.or(fanart),
                id
            ],
        )?;

        // 独立目录模式：把对应番号的 .strm 指向新视频路径（外部媒体库点播才不会失效）
        if let Err(e) = crate::media::storage::sync_independent_strm(
            &storage_cfg,
            local_id.as_deref().unwrap_or_default(),
            &new_path_str,
        ) {
            log::warn!("[video_move] event=sync_strm_failed video_id={} error={}", id, e);
        }

        Ok(())
    })
    .await
    .map_err(|e| AppError::TaskJoin(e.to_string()))?
}

#[tauri::command]
pub async fn update_video(app: AppHandle, db: State<'_, crate::db::Database>, id: String, data: VideoUpdatePayload) -> AppResult<VideoUpdateResult> {
    // 确保视频在独立的同名目录中（避免重命名时影响其他视频的资源）
    if let Err(e) = ensure_video_in_own_dir_with_db(&app, &id) {
        log::warn!(
            "[video_update] event=ensure_own_dir_failed video_id={} error={}",
            id,
            e
        );
    }

    let mut conn = db.get_connection()?;
    let app_clone = app.clone();
    // 独立目录模式配置（用于重命名后同步 .strm）
    let storage_cfg = crate::media::storage::MetadataStorageConfig::from_settings(
        &crate::settings::get_settings(app.clone()).await.unwrap_or_default(),
    );

    tokio::task::spawn_blocking(move || {
        let _app = app_clone;

        let title_to_store = data.title.as_ref().map(|value| value.trim().to_string());

        if matches!(title_to_store.as_deref(), Some("")) {
            return Err(AppError::Business("标题不能为空".to_string()));
        }

        let current = conn
            .query_row(
                "SELECT title, original_title, local_id, studio, director, premiered, duration, rating, video_path, dir_path, poster, thumb, fanart FROM videos WHERE id = ?",
                [&id],
                |row| {
                    Ok(VideoUpdateContext {
                        title: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                        original_title: row.get(1)?,
                        local_id: row.get(2)?,
                        studio: row.get(3)?,
                        director: row.get(4)?,
                        premiered: row.get(5)?,
                        duration: row.get(6)?,
                        rating: row.get(7)?,
                        video_path: row.get(8)?,
                        dir_path: row.get(9)?,
                        poster: row.get(10)?,
                        thumb: row.get(11)?,
                        fanart: row.get(12)?,
                        actors: Vec::new(),
                        tags: Vec::new(),
                        genres: Vec::new(),
                    })
                },
            )?;

        let mut current = VideoUpdateContext {
            actors: load_video_relation_names(
                &conn,
                "SELECT a.name FROM video_actors va JOIN actors a ON va.actor_id = a.id WHERE va.video_id = ? ORDER BY va.priority",
                &id,
            ).map_err(|e| AppError::Business(e))?,
            tags: load_video_relation_names(
                &conn,
                "SELECT t.name FROM video_tags vt JOIN tags t ON vt.tag_id = t.id WHERE vt.video_id = ? ORDER BY t.name",
                &id,
            ).map_err(|e| AppError::Business(e))?,
            genres: load_video_relation_names(
                &conn,
                "SELECT g.name FROM video_genres vg JOIN genres g ON vg.genre_id = g.id WHERE vg.video_id = ? ORDER BY g.name",
                &id,
            ).map_err(|e| AppError::Business(e))?,
            ..current
        };

        let mut parsed_duration = current.duration.map(|value| value as i32);
        let nfo_path = std::path::Path::new(&current.video_path).with_extension("nfo");
        let parsed_nfo = if nfo_path.exists() {
            crate::nfo::parser::parse_nfo(&nfo_path, &mut parsed_duration)
        } else {
            None
        };

        let updated_actors = data.actors.as_ref().map(|actors| parse_name_list(actors));
        let updated_tags = data.tags.as_ref().map(|tags| parse_name_list(tags));
        let rewritten_nfo_metadata = build_nfo_metadata_for_update(
            &current,
            &data,
            parsed_nfo.as_ref(),
            updated_actors.as_deref(),
            updated_tags.as_deref(),
        );
        let final_video_path = current.video_path.clone();
        let final_dir_path = current.dir_path.clone().or_else(|| {
            std::path::Path::new(&current.video_path)
                .parent()
                .map(|path| path.to_string_lossy().to_string())
        });
        let final_poster = current.poster.clone();
        let final_thumb = current.thumb.clone();
        let final_fanart = current.fanart.clone();

        let tx = conn.transaction()?;

        // 更新基本字段
        let mut sql_parts = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(v) = &title_to_store {
            sql_parts.push("title = ?");
            params.push(Box::new(v.clone()) as Box<dyn rusqlite::ToSql>);
            current.title = v.clone();
        }
        if let Some(v) = &data.local_id {
            sql_parts.push("local_id = ?");
            params.push(Box::new(v.clone()) as Box<dyn rusqlite::ToSql>);
        }
        if let Some(v) = &data.duration {
            sql_parts.push("duration = ?");
            params.push(Box::new(*v as i64) as Box<dyn rusqlite::ToSql>);
        }
        if let Some(v) = &data.premiered {
            sql_parts.push("premiered = ?");
            params.push(Box::new(v.clone()) as Box<dyn rusqlite::ToSql>);
        }
        if let Some(v) = &data.rating {
            sql_parts.push("rating = ?");
            params.push(Box::new(*v) as Box<dyn rusqlite::ToSql>);
        }

        // 直接字符串字段（不再使用外键）
        if let Some(v) = &data.studio {
            sql_parts.push("studio = ?");
            params.push(Box::new(v.clone()) as Box<dyn rusqlite::ToSql>);
        }
        if let Some(v) = &data.director {
            sql_parts.push("director = ?");
            params.push(Box::new(v.clone()) as Box<dyn rusqlite::ToSql>);
        }
        if let Some(v) = &data.resolution {
            sql_parts.push("resolution = ?");
            params.push(Box::new(v.clone()) as Box<dyn rusqlite::ToSql>);
        }
        // maker 字段已不再使用

        sql_parts.push("updated_at = datetime('now')");

        if !sql_parts.is_empty() {
            let sql = format!("UPDATE videos SET {} WHERE id = ?", sql_parts.join(", "));
            params.push(Box::new(id.clone()));

            let mut stmt = tx.prepare(&sql)?;
            stmt.execute(rusqlite::params_from_iter(params.iter()))?;
        }

        if let Some(actors) = &updated_actors {
            current.actors = actors.clone();

            tx.execute("DELETE FROM video_actors WHERE video_id = ?", [&id])?;

            for (idx, actor_name) in actors.iter().enumerate() {
                let actor_id = crate::db::Database::get_or_create_actor(&tx, actor_name)?;
                tx.execute(
                    "INSERT INTO video_actors (video_id, actor_id, priority) VALUES (?, ?, ?)",
                    rusqlite::params![&id, actor_id, idx as i64],
                )?;
            }
        }

        // 处理标签（如果提供）
        if let Some(tags) = &updated_tags {
            current.tags = tags.clone();

            // 删除已有标签
            tx.execute("DELETE FROM video_tags WHERE video_id = ?", [&id])?;

            // 插入新标签
            for tag_name in tags.iter() {
                let tag_id = crate::db::Database::get_or_create_tag(&tx, tag_name)?;
                tx.execute(
                    "INSERT INTO video_tags (video_id, tag_id) VALUES (?, ?)",
                    rusqlite::params![&id, tag_id],
                )?;
            }
        }

        // 维度同步：从更新后的行重建片商 / 系列 / 导演关联（番号/片商/导演任一变更都生效）
        {
            let (studio, director, local_id): (Option<String>, Option<String>, Option<String>) =
                tx.query_row(
                    "SELECT studio, director, local_id FROM videos WHERE id = ?",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?;
            crate::db::Database::sync_video_dimensions(
                &tx,
                &id,
                studio.as_deref(),
                director.as_deref(),
                local_id.as_deref(),
            )?;
        }

        // 详情页保存不再按标题重命名视频文件：保持磁盘上现有文件名（通常为番号），
        // 仅更新数据库字段与 NFO 内容。（刮削保存仍按各自逻辑处理文件名）

        // 独立目录模式：视频被重命名/移动时，同步对应番号的 .strm 指向新路径
        if final_video_path != current.video_path {
            if let Err(e) = crate::media::storage::sync_independent_strm(
                &storage_cfg,
                current.local_id.as_deref().unwrap_or_default(),
                &final_video_path,
            ) {
                log::warn!("[video_update] event=sync_strm_failed video_id={} error={}", id, e);
            }
        }

        // 独立目录模式：把更新后的 NFO 写回独立目录；非独立 / 未找到时回退写视频同级
        let wrote_independent_nfo = match crate::media::storage::save_nfo_to_independent_dir(
            &storage_cfg,
            current.local_id.as_deref().unwrap_or_default(),
            &rewritten_nfo_metadata,
        ) {
            Ok(wrote) => wrote,
            Err(e) => {
                log::warn!("[video_update] event=save_independent_nfo_failed video_id={} error={}", id, e);
                false
            }
        };
        if !wrote_independent_nfo {
            crate::media::assets::save_nfo_for_video(&final_video_path, &rewritten_nfo_metadata)
                .map_err(|e| AppError::Business(e))?;
        }

        tx.commit()?;

        Ok(VideoUpdateResult {
            title: current.title,
            video_path: final_video_path,
            dir_path: final_dir_path,
            poster: final_poster,
            thumb: final_thumb,
            fanart: final_fanart,
        })
    })
    .await
    .map_err(|e| AppError::TaskJoin(e.to_string()))?
}

#[tauri::command]
pub async fn find_ad_videos(
    app: AppHandle,
    db: State<'_, crate::db::Database>,
    keywords: Option<Vec<String>>,
    check_duplicate: Option<bool>,
    exclude_keywords: Option<Vec<String>>,
) -> AppResult<Vec<AdVideo>> {
    use std::collections::HashMap;

    let check_duplicate = check_duplicate.unwrap_or(true);

    // 如果没有传入关键词，从设置中读取（async）
    let settings = crate::settings::get_settings(app.clone()).await.map_err(|e| AppError::Business(e))?;
    let keywords = keywords.unwrap_or(settings.ad_filter.keywords);
    let exclude_keywords = exclude_keywords.unwrap_or(settings.ad_filter.exclude_keywords);

    log::info!(
        "[ad_video_scan] event=start keywords={:?} exclude_keywords={:?} check_duplicate={}",
        keywords,
        exclude_keywords,
        check_duplicate
    );

    let conn = db.get_connection()?;

    tokio::task::spawn_blocking(move || {
        let mut ad_videos = Vec::new();

        // 第一步：查询所有视频（移除 50MB 限制）
        let mut stmt = conn
            .prepare("SELECT id, video_path, file_size, title FROM videos")?;

        let all_videos = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?;

        let mut video_list = Vec::new();
        for video in all_videos {
            let (id, path, size, title) = video?;
            video_list.push((id, path, size, title));
        }

        log::info!(
            "[ad_video_scan] event=loaded_videos total={}",
            video_list.len()
        );

        // 第二步：统计文件名出现次数（在所有视频中统计）
        let mut filename_count: HashMap<String, Vec<String>> = HashMap::new();
        for (_, path, _, _) in &video_list {
            if let Some(filename) = std::path::Path::new(path).file_name() {
                let filename_str = filename.to_string_lossy().to_string();
                filename_count
                    .entry(filename_str.clone())
                    .or_insert_with(Vec::new)
                    .push(path.clone());
            }
        }

        // 第三步：检查每个视频
        for (id, path, size, title) in video_list {
            let filename = std::path::Path::new(&path)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();

            let mut reasons = Vec::new();

            // 规则1: 文件大小为0（优先级最高）
            if size == 0 {
                reasons.push("文件大小为 0".to_string());
            } else {
                // 规则2: 文件名重复2次及以上
                if check_duplicate {
                    if let Some(count) = filename_count.get(&filename) {
                        if count.len() >= 2 {
                            reasons.push(format!("文件名重复 {} 次", count.len()));
                        }
                    }
                }

                // 规则3: 关键词过滤（同时检查文件名和视频标题）
                let filename_lower = filename.to_lowercase();
                let title_lower = title.as_deref().unwrap_or("").to_lowercase();
                for keyword in &keywords {
                    let kw = keyword.to_lowercase();
                    if filename_lower.contains(&kw) || title_lower.contains(&kw) {
                        reasons.push(format!("包含关键词: {}", keyword));
                        break;
                    }
                }
            }

            // 如果有任何匹配的原因，添加到结果
            // 但如果文件名或标题包含排除关键词，则跳过
            if !reasons.is_empty() {
                let filename_lower = filename.to_lowercase();
                let title_lower = title.as_deref().unwrap_or("").to_lowercase();
                let excluded = exclude_keywords
                    .iter()
                    .any(|ek| {
                        let ek_lower = ek.to_lowercase();
                        filename_lower.contains(&ek_lower) || title_lower.contains(&ek_lower)
                    });
                if !excluded {
                    ad_videos.push(AdVideo {
                        id,
                        path: path.clone(),
                        filename,
                        file_size: size,
                        reason: reasons.join(", "),
                    });
                }
            }
        }

        log::info!(
            "[ad_video_scan] event=completed suspicious_total={}",
            ad_videos.len()
        );
        Ok(ad_videos)
    })
    .await
    .map_err(|e| AppError::TaskJoin(e.to_string()))?
}

/// 下载远程图片到 extrafanart 目录
#[tauri::command]
pub async fn download_remote_image(
    app: AppHandle,
    video_id: String,
    video_path: String,
    url: String,
) -> AppResult<String> {
    // 分离模式：预览图写入 <root>/<番号 标题>/extrafanart/，否则视频同级
    let settings = crate::settings::get_settings(app.clone()).await.unwrap_or_default();
    let cfg = crate::media::storage::MetadataStorageConfig::from_settings(&settings);
    let (local_id, title) = crate::db::Database::new(&app)
        .ok()
        .and_then(|db| {
            db.get_connection().ok().and_then(|conn| {
                conn.query_row(
                    "SELECT local_id, title FROM videos WHERE id = ?",
                    [&video_id],
                    |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
                )
                .ok()
            })
        })
        .map(|(a, b)| (a.unwrap_or_default(), b.unwrap_or_default()))
        .unwrap_or_default();
    let target = crate::media::storage::resolve_asset_target(&video_path, &local_id, &title, &cfg)
        .map_err(AppError::Business)?;
    let _ = crate::media::storage::ensure_asset_dir_and_strm(&target);
    let save_dir = crate::media::assets::extrafanart_dir_in(&target.dir);
    std::fs::create_dir_all(&save_dir)?;

    let next_index = crate::media::assets::next_extrafanart_index_in(&target.dir);
    let save_path = save_dir.join(format!("fanart{}.jpg", next_index));
    let client = crate::utils::proxy::apply_proxy_auto(
        wreq::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36"),
    )
    .map_err(|e| AppError::Business(format!("创建 HTTP 客户端失败: {}", e)))?
    .build()?;

    let resp = client
        .get(&url)
        .header(
            "Accept",
            "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8",
        )
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("Referer", "https://memojav.com/")
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(AppError::Business(format!("下载失败，HTTP 状态码: {}", resp.status())));
    }

    let bytes = resp
        .bytes()
        .await?;
    if bytes.is_empty() {
        return Err(AppError::Business("下载的数据为空".to_string()));
    }

    std::fs::write(&save_path, &bytes)?;

    Ok(save_path.to_string_lossy().to_string())
}

// 批量删除视频（复用 delete_video_and_files）
#[tauri::command]
pub async fn delete_videos(
    db: State<'_, crate::db::Database>,
    ids: Vec<String>,
    delete_scrape_data_only: Option<bool>,
) -> AppResult<()> {
    let conn = db.get_connection()?;
    let delete_scrape_data_only = delete_scrape_data_only.unwrap_or(false);

    tokio::task::spawn_blocking(move || {
        for id in ids {
            let result = if delete_scrape_data_only {
                clear_video_scrape_data(&conn, &id)
            } else {
                delete_video_and_files(&conn, &id)
            };

            if let Err(e) = result {
                log::error!(
                    "[video_delete] event=batch_delete_failed video_id={} delete_scrape_data_only={} error={}",
                    id,
                    delete_scrape_data_only,
                    e
                );
            }
        }

        let _ = update_all_directories_count(&conn);

        Ok(())
    })
    .await
    .map_err(|e| AppError::TaskJoin(e.to_string()))?
}

/// 库健康诊断：各诊断项计数
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryHealth {
    /// 视频总数
    pub total: i64,
    /// 识别失败（scan_status=3）
    pub recognize_failed: i64,
    /// 刮削失败（scan_status=4）
    pub scrape_failed: i64,
    /// 缺封面（poster/thumb/fanart 均空）
    pub missing_cover: i64,
    /// 缺 NFO（nfo_mtime 空）
    pub missing_nfo: i64,
    /// 缺番号（local_id 空，无法刮削/取图）
    pub missing_code: i64,
}

/// 聚合媒体库的元数据缺口与失败项计数（库健康诊断总览）。
#[tauri::command]
pub async fn get_library_health(db: State<'_, crate::db::Database>) -> AppResult<LibraryHealth> {
    let db = db.inner().clone();
    tokio::task::spawn_blocking(move || -> AppResult<LibraryHealth> {
        let conn = db.get_connection()?;
        let health = conn.query_row(
            "SELECT
                COUNT(*),
                SUM(CASE WHEN scan_status = 3 THEN 1 ELSE 0 END),
                SUM(CASE WHEN scan_status = 4 THEN 1 ELSE 0 END),
                SUM(CASE WHEN (poster IS NULL OR poster = '')
                          AND (thumb IS NULL OR thumb = '')
                          AND (fanart IS NULL OR fanart = '') THEN 1 ELSE 0 END),
                SUM(CASE WHEN nfo_mtime IS NULL THEN 1 ELSE 0 END),
                SUM(CASE WHEN local_id IS NULL OR local_id = '' THEN 1 ELSE 0 END)
             FROM videos",
            [],
            |row| {
                Ok(LibraryHealth {
                    total: row.get(0)?,
                    recognize_failed: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    scrape_failed: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    missing_cover: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    missing_nfo: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                    missing_code: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
                })
            },
        )?;
        Ok(health)
    })
    .await
    .map_err(|e| AppError::TaskJoin(e.to_string()))?
}
