use super::*;
use rusqlite::{params, Connection, OptionalExtension, Result};

impl Database {
    /// 检查视频是否有封面图
    pub fn has_cover_image(&self, video_path: &str) -> Result<bool> {
        let conn = self.get_connection()?;
        // 任一标准图集列存在即视为有封面（竖裁失败时可能仅有横版 fanart/thumb）
        let has_cover: bool = conn
            .query_row(
                "SELECT (poster IS NOT NULL AND poster <> '')
                     OR (fanart IS NOT NULL AND fanart <> '')
                     OR (thumb IS NOT NULL AND thumb <> '')
                 FROM videos WHERE video_path = ?1",
                params![video_path],
                |row| row.get(0),
            )
            .unwrap_or(false);
        Ok(has_cover)
    }

    pub fn update_video_cover_paths(
        conn: &Connection,
        video_id: &str,
        poster_path: Option<&str>,
        thumb_path: Option<&str>,
        fanart_path: Option<&str>,
        cover_width: Option<i32>,
        cover_height: Option<i32>,
    ) -> Result<()> {
        // 封面路径变更后旧缩略图内容作废，置空 cover_thumb 交由回填重新生成
        conn.execute(
            "UPDATE videos SET poster = ?, thumb = ?, fanart = ?, cover_width = ?, cover_height = ?, cover_thumb = NULL, updated_at = datetime('now') WHERE id = ?",
            rusqlite::params![poster_path, thumb_path, fanart_path, cover_width, cover_height, video_id],
        )?;
        Ok(())
    }

    pub fn get_video_duration(conn: &Connection, video_id: &str) -> Result<Option<i32>> {
        conn.query_row(
            "SELECT duration FROM videos WHERE id = ?",
            [video_id],
            |row| row.get(0),
        )
    }

    pub fn update_video_scrape_info(
        conn: &Connection,
        video_id: &str,
        data: &VideoScrapeUpdateData,
    ) -> Result<()> {
        conn.execute(
            "UPDATE videos SET
                title = ?,
                original_title = ?,
                studio = ?,
                director = ?,
                premiered = ?,
                duration = ?,
                rating = ?,
                poster = ?,
                thumb = ?,
                fanart = ?,
                local_id = ?,
                cover_width = ?,
                cover_height = ?,
                is_uncensored = ?,
                cover_thumb = NULL,
                has_subtitle = CASE WHEN ? THEN 1 ELSE has_subtitle END,
                scan_status = 2,
                scraped_at = datetime('now'),
                updated_at = datetime('now')
            WHERE id = ?",
            rusqlite::params![
                data.title,
                data.original_title.unwrap_or(data.title),
                data.studio,
                data.director,
                data.premiered,
                data.duration,
                data.rating.unwrap_or(0.0),
                data.poster,
                data.thumb,
                data.fanart,
                data.local_id,
                data.cover_width,
                data.cover_height,
                data.is_uncensored as i32,
                data.subtitle_saved,
                video_id
            ],
        )?;
        Ok(())
    }

    /// 更新「是否有字幕」标记（字幕手动下载成功 / 扫描探测后维护，列表不再实时探测文件系统）
    pub fn set_video_has_subtitle(conn: &Connection, video_path: &str, has_subtitle: bool) -> Result<()> {
        conn.execute(
            "UPDATE videos SET has_subtitle = ? WHERE video_path = ?",
            params![has_subtitle, video_path],
        )?;
        Ok(())
    }

    /// 更新文件创建时间（扫描发现旧库缺失时补写，列表按此排序、不再实时 stat）
    pub fn set_video_file_ctime(conn: &Connection, video_path: &str, file_ctime: Option<i64>) -> Result<()> {
        conn.execute(
            "UPDATE videos SET file_ctime = ? WHERE video_path = ?",
            params![file_ctime, video_path],
        )?;
        Ok(())
    }

    pub fn update_video_file_location(
        conn: &Connection,
        video_id: &str,
        old_video_path: &str,
        new_video_path: &str,
        new_dir_path: &str,
        poster: Option<&str>,
        thumb: Option<&str>,
        fanart: Option<&str>,
    ) -> Result<()> {
        conn.execute(
            "UPDATE videos SET video_path = ?, dir_path = ?, poster = ?, thumb = ?, fanart = ?, updated_at = datetime('now') WHERE id = ?",
            rusqlite::params![new_video_path, new_dir_path, poster, thumb, fanart, video_id],
        )?;

        conn.execute(
            "UPDATE scrape_tasks SET path = ? WHERE path = ?",
            rusqlite::params![new_video_path, old_video_path],
        )?;

        Ok(())
    }

    pub fn update_video_file_location_tx(
        conn: &rusqlite::Transaction,
        video_id: &str,
        old_video_path: &str,
        new_video_path: &str,
        new_dir_path: &str,
        poster: Option<&str>,
        thumb: Option<&str>,
        fanart: Option<&str>,
    ) -> Result<()> {
        conn.execute(
            "UPDATE videos SET video_path = ?, dir_path = ?, poster = ?, thumb = ?, fanart = ?, updated_at = datetime('now') WHERE id = ?",
            rusqlite::params![new_video_path, new_dir_path, poster, thumb, fanart, video_id],
        )?;

        conn.execute(
            "UPDATE scrape_tasks SET path = ? WHERE path = ?",
            rusqlite::params![new_video_path, old_video_path],
        )?;

        Ok(())
    }

    /// 分段影片随组搬进组目录后，同步同组其它段的库内路径（按原路径定位，`moved` 为原路径 → 新路径）。
    /// 图集路径原在该段所在目录下的一并改写到新目录（图随段搬走了）；尚未入库的段跳过，等下次扫描按新位置入库。
    pub fn update_stack_sibling_locations(
        conn: &Connection,
        new_dir_path: &str,
        moved: &[(String, String)],
    ) -> Result<()> {
        for (old_video_path, new_video_path) in moved {
            let row: Option<(String, Option<String>, Option<String>, Option<String>)> = conn
                .query_row(
                    "SELECT id, poster, thumb, fanart FROM videos WHERE video_path = ?1",
                    params![old_video_path],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;
            let Some((video_id, poster, thumb, fanart)) = row else {
                continue;
            };
            let old_dir = std::path::Path::new(old_video_path).parent();
            let remap = |asset: Option<String>| -> Option<String> {
                asset.map(|path| {
                    match old_dir.and_then(|dir| std::path::Path::new(&path).strip_prefix(dir).ok()) {
                        Some(rest) => std::path::Path::new(new_dir_path)
                            .join(rest)
                            .to_string_lossy()
                            .to_string(),
                        None => path,
                    }
                })
            };
            Self::update_video_file_location(
                conn,
                &video_id,
                old_video_path,
                new_video_path,
                new_dir_path,
                remap(poster).as_deref(),
                remap(thumb).as_deref(),
                remap(fanart).as_deref(),
            )?;
        }
        Ok(())
    }

    /// 预加载目录下所有已有视频的扫描信息到 HashMap，避免逐个查询
    pub fn get_existing_video_scan_info_map(
        conn: &Connection,
        dir_path: &str,
    ) -> Result<std::collections::HashMap<String, ExistingVideoScanInfo>> {
        let mut stmt = conn.prepare(
            "SELECT
                video_path, id, title, original_title, studio, premiered, director,
                local_id, rating, file_size, fast_hash, duration, resolution,
                file_mtime, nfo_mtime, poster_mtime, thumb_mtime, fanart_mtime,
                poster, thumb, fanart, scan_status, stack_key, part_index, has_subtitle, file_ctime
            FROM videos
            WHERE dir_path LIKE ? || '%'"
        )?;
        let rows = stmt.query_map([dir_path], |row| {
            Ok((
                row.get::<_, String>(0)?,
                ExistingVideoScanInfo {
                    id: row.get(1)?,
                    title: row.get(2)?,
                    original_title: row.get(3)?,
                    studio: row.get(4)?,
                    premiered: row.get(5)?,
                    director: row.get(6)?,
                    local_id: row.get(7)?,
                    rating: row.get(8)?,
                    file_size: row.get::<_, Option<i64>>(9)?.unwrap_or(0) as u64,
                    fast_hash: row.get(10)?,
                    duration: row.get(11)?,
                    resolution: row.get(12)?,
                    file_mtime: row.get(13)?,
                    nfo_mtime: row.get(14)?,
                    poster_mtime: row.get(15)?,
                    thumb_mtime: row.get(16)?,
                    fanart_mtime: row.get(17)?,
                    poster: row.get(18)?,
                    thumb: row.get(19)?,
                    fanart: row.get(20)?,
                    scan_status: row.get::<_, Option<i32>>(21)?.unwrap_or(1),
                    stack_key: row.get(22)?,
                    part_index: row.get(23)?,
                    has_subtitle: row.get::<_, Option<i64>>(24)?.map(|v| v != 0),
                    file_ctime: row.get(25)?,
                },
            ))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 批量删除视频记录（按路径列表）
    pub fn batch_delete_videos_by_paths(conn: &rusqlite::Transaction, paths: &[&str]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        // 分批处理，SQLite 参数上限为 999
        for chunk in paths.chunks(500) {
            let placeholders: Vec<&str> = chunk.iter().map(|_| "?").collect();
            let sql = format!(
                "DELETE FROM videos WHERE video_path IN ({})",
                placeholders.join(",")
            );
            let params: Vec<&dyn rusqlite::types::ToSql> = chunk
                .iter()
                .map(|s| s as &dyn rusqlite::types::ToSql)
                .collect();
            conn.execute(&sql, params.as_slice())?;
        }
        Ok(())
    }

    /// 根据番号 (local_id) 获取已存在的视频信息 (包含 id, title, video_path 等)
    pub async fn get_video_by_local_id(&self, local_id: &str) -> AppResult<Option<serde_json::Value>> {
        let local_id_upper = local_id.to_uppercase();

        self.run_blocking(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, title, video_path, file_size
                 FROM videos WHERE local_id = ?1 COLLATE NOCASE",
            )?;

            let mut rows = stmt.query(params![local_id_upper])?;

            if let Some(row) = rows.next()? {
                let id: String = row.get(0)?;
                let title: String = row.get(1)?;
                let video_path: String = row.get(2)?;
                let file_size: Option<i64> = row.get(3)?;

                Ok(Some(serde_json::json!({
                    "id": id,
                    "title": title,
                    "videoPath": video_path,
                    "fileSize": file_size
                })))
            } else {
                Ok(None)
            }
        }).await
    }

    pub fn get_video_id_by_path(conn: &rusqlite::Transaction, video_path: &str) -> Result<String> {
        conn.query_row(
            "SELECT id FROM videos WHERE video_path = ?",
            params![video_path],
            |r| r.get(0),
        )
    }

    pub fn update_video(conn: &rusqlite::Transaction, data: &VideoUpdateData) -> Result<()> {
        conn.execute(
            "UPDATE videos SET
                updated_at = ?2,
                title = ?3,
                studio = ?4,
                premiered = ?5,
                director = ?6,
                file_size = ?7,
                fast_hash = ?8,
                original_title = ?9,
                duration = ?10,
                resolution = ?11,
                local_id = ?12,
                rating = ?13,
                poster = ?14,
                thumb = ?15,
                fanart = ?16,
                file_mtime = ?17,
                nfo_mtime = ?18,
                poster_mtime = ?19,
                thumb_mtime = ?20,
                fanart_mtime = ?21,
                scan_status = ?22,
                stack_key = ?23,
                part_index = ?24,
                has_subtitle = ?25,
                file_ctime = ?26
            WHERE video_path = ?1",
            params![
                data.path_str,
                data.now,
                data.title,
                data.studio,
                data.premiered,
                data.director,
                data.file_size as i64,
                data.fast_hash,
                data.original_title,
                data.duration,
                data.resolution,
                data.local_id,
                data.rating,
                data.poster,
                data.thumb,
                data.fanart,
                data.file_mtime,
                data.nfo_mtime,
                data.poster_mtime,
                data.thumb_mtime,
                data.fanart_mtime,
                data.scan_status,
                data.stack_key,
                data.part_index,
                data.has_subtitle,
                data.file_ctime
            ],
        )?;
        Ok(())
    }

    pub fn insert_video(conn: &rusqlite::Transaction, data: &VideoInsertData) -> Result<()> {
        conn.execute(
            "INSERT INTO videos (
                id, local_id, video_path, dir_path, title, original_title,
                studio, premiered, director,
                file_size, fast_hash, created_at, updated_at, scan_status,
                duration, resolution, rating, poster, thumb, fanart,
                file_mtime, nfo_mtime, poster_mtime, thumb_mtime, fanart_mtime,
                cover_width, cover_height, stack_key, part_index, has_subtitle, file_ctime
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30)",
            params![
                data.id,
                data.local_id,
                data.path_str,
                data.parent_str,
                data.title,
                data.original_title,
                data.studio,
                data.premiered,
                data.director,
                data.file_size as i64,
                data.fast_hash,
                data.created_at,
                data.scan_status,
                data.duration,
                data.resolution,
                data.rating,
                data.poster,
                data.thumb,
                data.fanart,
                data.file_mtime,
                data.nfo_mtime,
                data.poster_mtime,
                data.thumb_mtime,
                data.fanart_mtime,
                data.cover_width,
                data.cover_height,
                data.stack_key,
                data.part_index,
                data.has_subtitle,
                data.file_ctime
            ],
        )?;
        Ok(())
    }

    pub fn clear_video_actors(conn: &rusqlite::Transaction, video_id: &str) -> Result<()> {
        conn.execute(
            "DELETE FROM video_actors WHERE video_id = ?",
            params![video_id],
        )?;
        Ok(())
    }

    pub fn add_video_actor(
        conn: &rusqlite::Transaction,
        video_id: &str,
        actor_id: i64,
        priority: usize,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO video_actors (video_id, actor_id, priority) VALUES (?, ?, ?)",
            params![video_id, actor_id, priority as i64],
        )?;
        Ok(())
    }

    pub fn clear_video_tags(conn: &rusqlite::Transaction, video_id: &str) -> Result<()> {
        conn.execute(
            "DELETE FROM video_tags WHERE video_id = ?",
            params![video_id],
        )?;
        Ok(())
    }

    pub fn add_video_tag(conn: &rusqlite::Transaction, video_id: &str, tag_id: i64) -> Result<()> {
        conn.execute(
            "INSERT INTO video_tags (video_id, tag_id) VALUES (?, ?)",
            params![video_id, tag_id],
        )?;
        Ok(())
    }

    pub fn clear_video_genres(conn: &rusqlite::Transaction, video_id: &str) -> Result<()> {
        conn.execute(
            "DELETE FROM video_genres WHERE video_id = ?",
            params![video_id],
        )?;
        Ok(())
    }

    pub fn add_video_genre(
        conn: &rusqlite::Transaction,
        video_id: &str,
        genre_id: i64,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO video_genres (video_id, genre_id) VALUES (?, ?)",
            params![video_id, genre_id],
        )?;
        Ok(())
    }

    pub fn get_video_scan_status_by_path(
        conn: &Connection,
        video_path: &str,
    ) -> Result<Option<i32>> {
        conn.query_row(
            "SELECT scan_status FROM videos WHERE video_path = ?",
            [video_path],
            |row| row.get(0),
        )
        .optional()
    }

    /// 仅更新扫描状态（扫描时自愈历史误判用，避免全量 update_video 的开销）
    pub fn update_video_scan_status(
        conn: &rusqlite::Transaction,
        video_id: &str,
        scan_status: i32,
    ) -> Result<()> {
        conn.execute(
            "UPDATE videos SET scan_status = ?, updated_at = datetime('now') WHERE id = ?",
            params![scan_status, video_id],
        )?;
        Ok(())
    }

    /// 仅更新分段归并键与段序号（扫描时自愈历史分段误判用，避免全量 update_video 的开销）
    pub fn update_video_stack(
        conn: &rusqlite::Transaction,
        video_id: &str,
        stack_key: Option<&str>,
        part_index: Option<i64>,
    ) -> Result<()> {
        conn.execute(
            "UPDATE videos SET stack_key = ?, part_index = ?, updated_at = datetime('now') WHERE id = ?",
            params![stack_key, part_index, video_id],
        )?;
        Ok(())
    }
}
