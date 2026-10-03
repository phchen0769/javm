// 目录相关类型定义

/** 目录信息 */
export interface Directory {
    id: string
    path: string
    videoCount: number
    /** 是否启用：禁用后媒体库不显示该目录的视频、扫描跳过该目录 */
    enabled: boolean
    createdAt: string
    updatedAt: string
}
