// 资源搜索相关类型定义

/** 资源搜索结果项 */
export interface ResourceItem {
  code: string          // 番号
  title: string         // 名称
  actors: string        // 演员（逗号分隔）
  detailLevel?: string  // 数据丰富度标签
  detailScore?: number  // 数据丰富度评分
  duration: string      // 时长（如 "120分钟"）
  studio: string        // 制作商
  source?: string       // 数据来源（数据源名称）
  pageUrl?: string      // 详情页链接地址
  coverUrl?: string     // 封面图 URL（可能是本地缓存路径）
  remoteCoverUrl?: string // 原始远程封面 URL（代理后保留）
  remoteThumbs?: string[] // 原始远程预览图 URL（代理后保留）
  director?: string     // 导演
  tags?: string         // 标签/分类（逗号分隔）
  premiered?: string    // 发行日期
  rating?: number       // 评分
  thumbs?: string[] // 预览图 URL 列表
}

/** 数据源定义 */
export interface DataSource {
  name: string                                  // 数据源名称
  buildUrl: (code: string) => string            // URL 构建函数
  parse: (html: string) => ResourceItem | null  // HTML 解析函数
}

/** 单个刮削源的执行诊断（信息来自哪个网址、是否有效） */
export interface SourceDiagnostic {
  source: string        // 源 id（与设置里的站点 id 对应，便于关闭）
  siteName: string      // 真实站点名
  url: string           // 命中详情页地址（成功时）或站点主页
  status: 'success' | 'empty' | 'failed' | 'timeout'  // 成功/无数据/失败/太慢
  error?: string        // 失败原因（status=failed 时有值）
  elapsedMs: number     // 耗时（毫秒）
}

/** 融合刮削响应：最佳结果 + 各源诊断 */
export interface FusedScrapeResult {
  result: ResourceItem | null
  diagnostics: SourceDiagnostic[]
}
