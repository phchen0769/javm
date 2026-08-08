import { convertFileSrc } from '@tauri-apps/api/core'

export function isTauriRuntime() {
  return typeof window !== 'undefined' && Boolean((window as any).__TAURI_INTERNALS__)
}

/** 判断路径是否为本地文件路径（非 http/data/blob） */
export function isLocalPath(path?: string | null): boolean {
  if (!path) return false
  const trimmed = path.trim()
  if (!trimmed) return false
  if (trimmed.startsWith('//')) return false
  return !/^(https?:|data:|blob:)/i.test(trimmed)
}

export function toImageSrc(path?: string | null): string | null {
  if (!path) return null
  const trimmed = path.trim()
  if (!trimmed) return null
  if (trimmed.startsWith('//')) return `https:${trimmed}`
  if (/^(https?:|data:|blob:)/i.test(trimmed)) return trimmed
  if (!isTauriRuntime()) return null
  return convertFileSrc(trimmed.replace(/\\/g, '/'))
}

/** 封面图集字段（竖版 poster / 横版 thumb / 横版 fanart / 网格小缩略图 coverThumb） */
export interface CoverImageFields {
  poster?: string
  thumb?: string
  fanart?: string
  coverThumb?: string
}

/**
 * 按封面方向偏好选择展示图（标准图集对齐）：
 * - 横屏（landscape，默认）：fanart → thumb → poster
 * - 竖屏（portrait）：poster → fanart → thumb
 *
 * 缺失时回退另一方向，保证任何布局都不留空白。
 *
 * `preferThumbnail` 为真时，横屏模式优先用网格小缩略图 `coverThumb`（大幅降低解码开销，
 * 媒体库网格专用）；竖屏模式不用它——它是横版小图，塞进竖版卡片比例不对。
 */
export function resolveCoverImage(
  video: CoverImageFields,
  coverType?: string,
  preferThumbnail = false,
): string | undefined {
  if (coverType === 'portrait') {
    return video.poster || video.fanart || video.thumb || undefined
  }
  if (preferThumbnail && video.coverThumb) {
    return video.coverThumb
  }
  return video.fanart || video.thumb || video.poster || undefined
}

/** 是否存在任意封面图（poster / thumb / fanart） */
export function hasCoverImage(video: CoverImageFields): boolean {
  return Boolean(video.poster || video.thumb || video.fanart)
}

/**
 * 等高画廊（瀑布流）单图宽高比（宽/高）。
 *
 * 仅当存储的封面尺寸方向与当前 `coverType` 期望方向一致时采用真实尺寸（横版瀑布流的参差感），
 * 否则回退到布局默认比例 `fallbackRatio`——避免「竖屏模式用横版尺寸把竖版海报塞进宽卡片」。
 */
export function galleryCoverRatio(
  dims: { coverWidth?: number; coverHeight?: number },
  coverType: string | undefined,
  fallbackRatio: number,
): number {
  const w = dims.coverWidth
  const h = dims.coverHeight
  if (w && h && h > 0) {
    const imgIsPortrait = h > w
    const wantPortrait = coverType === 'portrait'
    if (imgIsPortrait === wantPortrait) return w / h
  }
  return fallbackRatio
}
