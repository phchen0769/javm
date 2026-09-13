// 视频状态管理
import { defineStore } from 'pinia'
import { ref, shallowRef, computed } from 'vue'
import type { Video, VideoFilter, Publisher, Tag, Directory } from '@/types'
import { getVideos } from '@/lib/tauri'

export const useVideoStore = defineStore('video', () => {
    // ============ State ============
    // 列表只读展示，用 shallowRef 避免上千个 Video 对象被逐字段深度代理；
    // 所有写操作都整体替换数组（见 fetchVideos/updateVideo/removeVideo）以触发更新。
    const videos = shallowRef<Video[]>([])
    const publishers = ref<Publisher[]>([]) // Was studios
    const tags = ref<Tag[]>([])
    const directories = ref<Directory[]>([])
    const filter = ref<VideoFilter>({
        sortBy: 'title',
        sortOrder: 'desc',
    })
    const loading = ref(false)
    const error = ref<string | null>(null)
    const selectedIds = ref<string[]>([])
    const coverVersions = ref<Record<string, number>>({})
    let refreshTimer: ReturnType<typeof setTimeout> | null = null
    let scheduledRefreshIncludesDirectories = false
    let lastRefreshAt = 0
    // 批量刮削时每个任务完成都会请求刷新，而单次整库刷新含数千次文件系统 stat；
    // 故把刷新节流为「最多每 REFRESH_MIN_INTERVAL 跑一次」，避免刷新风暴打满 CPU。
    const REFRESH_MIN_INTERVAL = 4000
    // 列表接口正常只需几十毫秒；超过此时长视为本次 IPC 响应丢失，放弃等待，
    // 否则 loading 永远为 true，刷新按钮会一直处于禁用状态
    const FETCH_VIDEOS_TIMEOUT_MS = 20000

    const normalizePath = (path?: string) => (path || '').replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase()

    // ============ Getters ============
    const filteredVideos = computed(() => {
        let result = [...videos.value]

        // 搜索过滤 - 支持任意 UTF-8 字符（中文、日文、韩文等）
        if (filter.value.search) {
            const search = filter.value.search.trim()
            if (search) {
                // 对于 ASCII 字符使用不区分大小写搜索，对于其他字符（如中文）直接匹配
                const searchLower = search.toLowerCase()
                
                result = result.filter(v => {
                    // 搜索番号
                    const localId = (v.localId || '').toLowerCase()
                    if (localId.includes(searchLower)) return true
                    
                    // 搜索标题（支持中文等 UTF-8 字符）
                    const title = v.title || ''
                    if (title.toLowerCase().includes(searchLower) || title.includes(search)) return true
                    
                    // 搜索原始标题
                    const originalTitle = v.originalTitle || ''
                    if (originalTitle.toLowerCase().includes(searchLower) || originalTitle.includes(search)) return true
                    
                    // 搜索演员名称
                    const actors = v.actors || ''
                    if (actors.toLowerCase().includes(searchLower) || actors.includes(search)) return true
                    
                    // 搜索制作商
                    const studio = v.studio || ''
                    if (studio.toLowerCase().includes(searchLower) || studio.includes(search)) return true
                    
                    return false
                })
            }
        }

        if (filter.value.directoryPath) {
            const selectedDirectory = normalizePath(filter.value.directoryPath)

            result = result.filter(v => {
                const videoDirectory = normalizePath(v.dirPath || v.videoPath.replace(/[\\/][^\\/]+$/, ''))

                return videoDirectory === selectedDirectory || videoDirectory.startsWith(`${selectedDirectory}/`)
            })
        }

        // 评分过滤
        if (filter.value.minRating !== undefined) {
            result = result.filter(v => (v.rating ?? 0) >= filter.value.minRating!)
        }
        if (filter.value.maxRating !== undefined) {
            result = result.filter(v => (v.rating ?? 0) <= filter.value.maxRating!)
        }

        if (filter.value.fileCreatedAfter) {
            const threshold = new Date(filter.value.fileCreatedAfter).getTime()

            if (!Number.isNaN(threshold)) {
                result = result.filter(v => {
                    if (!v.fileCreatedAt) {
                        return false
                    }

                    const fileCreatedTime = new Date(v.fileCreatedAt).getTime()
                    return !Number.isNaN(fileCreatedTime) && fileCreatedTime >= threshold
                })
            }
        }

        // 分辨率过滤
        if (filter.value.resolution && filter.value.resolution.length > 0) {
            result = result.filter(v => {
                if (!v.resolution) return false
                
                // 将数据库中的分辨率格式 (如 "1920x1080") 转换为标准格式 (如 "1080p")
                const normalizeResolution = (res: string): string => {
                    const match = res.match(/(\d+)x(\d+)/)
                    if (!match) return res
                    
                    // const width = parseInt(match[1])
                    const height = parseInt(match[2])
                    
                    // 根据高度判断分辨率等级
                    if (height >= 2160) return '4K'
                    if (height >= 1080) return '1080p'
                    if (height >= 720) return '720p'
                    return 'SD'
                }
                
                const normalizedRes = normalizeResolution(v.resolution)
                return filter.value.resolution!.includes(normalizedRes)
            })
        }

        // 刮削状态过滤
        if (filter.value.scraped && filter.value.scraped.length > 0) {
            result = result.filter(v => {
                // 已刮削：状态为 Completed (2)
                const isScraped = v.scanStatus === 2
                
                const hasScraped = filter.value.scraped!.includes('scraped')
                const hasUnscraped = filter.value.scraped!.includes('unscraped')
                
                // 两个都选中，显示全部
                if (hasScraped && hasUnscraped) {
                    return true
                }
                
                // 只选中已刮削
                if (hasScraped && !hasUnscraped) {
                    return isScraped
                }
                
                // 只选中未刮削
                if (!hasScraped && hasUnscraped) {
                    return !isScraped
                }
                
                return true
            })
        }

        // 有码/无码过滤
        if (filter.value.censorship && filter.value.censorship.length > 0) {
            result = result.filter(v => {
                const isUncensored = v.isUncensored === true
                const hasCensored = filter.value.censorship!.includes('censored')
                const hasUncensored = filter.value.censorship!.includes('uncensored')
                if (hasCensored && hasUncensored) return true
                if (hasCensored && !hasUncensored) return !isUncensored
                if (!hasCensored && hasUncensored) return isUncensored
                return true
            })
        }

        // 库类型过滤：标准库(有番号) / 非标准库(无番号)
        if (filter.value.libraryType && filter.value.libraryType.length > 0) {
            result = result.filter(v => {
                const isStandard = !!(v.localId && v.localId.trim() !== '')
                const hasStandard = filter.value.libraryType!.includes('standard')
                const hasNonStandard = filter.value.libraryType!.includes('nonStandard')
                if (hasStandard && hasNonStandard) return true
                if (hasStandard && !hasNonStandard) return isStandard
                if (!hasStandard && hasNonStandard) return !isStandard
                return true
            })
        }

        // 状态过滤
        if (filter.value.status !== undefined) {
            result = result.filter(v => v.scanStatus === filter.value.status)
        }

        // 排序
        if (filter.value.sortBy) {
            result.sort((a, b) => {
                let aVal: any = a[filter.value.sortBy as keyof Video]
                let bVal: any = b[filter.value.sortBy as keyof Video]

                // 处理 undefined 和 null 值，放到最后
                const aIsEmpty = aVal === undefined || aVal === null
                const bIsEmpty = bVal === undefined || bVal === null

                if (aIsEmpty && bIsEmpty) return 0
                if (aIsEmpty) return 1
                if (bIsEmpty) return -1

                // 日期字段特殊处理：转换为时间戳比较
                if (filter.value.sortBy === 'createdAt' || filter.value.sortBy === 'fileCreatedAt' || filter.value.sortBy === 'premiered') {
                    const aTime = new Date(aVal).getTime()
                    const bTime = new Date(bVal).getTime()

                    // 处理无效日期
                    if (isNaN(aTime) && isNaN(bTime)) return 0
                    if (isNaN(aTime)) return 1
                    if (isNaN(bTime)) return -1

                    return filter.value.sortOrder === 'asc' ? aTime - bTime : bTime - aTime
                }

                // 数字字段特殊处理：确保转换为数字后比较
                if (filter.value.sortBy === 'duration' || filter.value.sortBy === 'rating' || filter.value.sortBy === 'fileSize') {
                    const aNum = typeof aVal === 'number' ? aVal : parseFloat(String(aVal))
                    const bNum = typeof bVal === 'number' ? bVal : parseFloat(String(bVal))

                    // 处理无效数字
                    if (isNaN(aNum) && isNaN(bNum)) return 0
                    if (isNaN(aNum)) return 1
                    if (isNaN(bNum)) return -1

                    return filter.value.sortOrder === 'asc' ? aNum - bNum : bNum - aNum
                }

                // 其他数字类型直接比较
                if (typeof aVal === 'number' && typeof bVal === 'number') {
                    return filter.value.sortOrder === 'asc' ? aVal - bVal : bVal - aVal
                }

                // 字符串类型：转小写后比较
                if (typeof aVal === 'string' && typeof bVal === 'string') {
                    const aLower = aVal.toLowerCase()
                    const bLower = bVal.toLowerCase()

                    if (filter.value.sortOrder === 'asc') {
                        return aLower > bLower ? 1 : aLower < bLower ? -1 : 0
                    }
                    return aLower < bLower ? 1 : aLower > bLower ? -1 : 0
                }

                // 其他情况：转字符串比较
                const aStr = String(aVal).toLowerCase()
                const bStr = String(bVal).toLowerCase()

                if (filter.value.sortOrder === 'asc') {
                    return aStr > bStr ? 1 : aStr < bStr ? -1 : 0
                }
                return aStr < bStr ? 1 : aStr > bStr ? -1 : 0
            })
        }

        return result
    })

    const totalCount = computed(() => videos.value.length)
    const filteredCount = computed(() => filteredVideos.value.length)

    const selectedVideos = computed(() =>
        videos.value.filter(v => selectedIds.value.includes(v.id))
    )

    // ============ Actions ============
    async function fetchVideos() {
        const isTauri = typeof window !== 'undefined' && Boolean((window as any).__TAURI_INTERNALS__)
        if (!isTauri) {
            videos.value = []
            return
        }
        loading.value = true
        error.value = null

        let timeoutTimer: ReturnType<typeof setTimeout> | undefined
        try {
            const previousVideos = new Map(videos.value.map(video => [video.id, video]))
            const timeout = new Promise<never>((_, reject) => {
                timeoutTimer = setTimeout(
                    () => reject(new Error('获取视频列表超时，后端未返回响应，请重试')),
                    FETCH_VIDEOS_TIMEOUT_MS,
                )
            })
            const fetchedVideos = await Promise.race([getVideos(), timeout])

            for (const nextVideo of fetchedVideos) {
                const previousVideo = previousVideos.get(nextVideo.id)
                if (!previousVideo) {
                    continue
                }

                // 仅在封面路径真正变化、或重新刮削（scrapedAt 变化，新封面会覆盖写回原路径）时才 bump
                //（触发前端换图并破缓存）。
                // 不要把 scanStatus 变化也算进来：批量刮削时状态 0→1→2 频繁跳动，
                // 会让大量卡片的封面版本号无谓地变，强制全尺寸封面重复解码，拖垮 CPU。
                if (
                    previousVideo.poster !== nextVideo.poster ||
                    previousVideo.thumb !== nextVideo.thumb ||
                    previousVideo.fanart !== nextVideo.fanart ||
                    previousVideo.scrapedAt !== nextVideo.scrapedAt
                ) {
                    bumpCoverVersion(nextVideo.id)
                }
            }

            videos.value = fetchedVideos
        } catch (e) {
            error.value = (e as Error).message
            console.error('Failed to fetch videos:', e)
        } finally {
            clearTimeout(timeoutTimer)
            loading.value = false
        }
    }

    function setFilter(newFilter: Partial<VideoFilter>) {
        filter.value = { ...filter.value, ...newFilter }
    }

    function resetFilter() {
        filter.value = {
            sortBy: 'title',
            sortOrder: 'desc',
        }
    }

    function selectVideo(id: string) {
        if (!selectedIds.value.includes(id)) {
            selectedIds.value.push(id)
        }
    }

    function deselectVideo(id: string) {
        selectedIds.value = selectedIds.value.filter(i => i !== id)
    }

    function toggleSelect(id: string) {
        if (selectedIds.value.includes(id)) {
            deselectVideo(id)
        } else {
            selectVideo(id)
        }
    }

    function selectAll() {
        selectedIds.value = filteredVideos.value.map(v => v.id)
    }

    function deselectAll() {
        selectedIds.value = []
    }

    function updateVideo(id: string, data: Partial<Video>) {
        const index = videos.value.findIndex(v => v.id === id)
        if (index !== -1) {
            // shallowRef 下需整体替换数组才能触发更新
            const next = videos.value.slice()
            next[index] = { ...next[index], ...data }
            videos.value = next
        }
    }

    function removeVideo(id: string) {
        videos.value = videos.value.filter(v => v.id !== id)
        deselectVideo(id)
    }

    function bumpCoverVersion(id: string) {
        coverVersions.value = { ...coverVersions.value, [id]: Date.now() }
    }

    async function refreshLibrary(includeDirectories = false) {
        await Promise.all([
            fetchVideos(),
            includeDirectories ? fetchDirectories() : Promise.resolve(),
        ])
    }

    function scheduleRefresh(options?: { includeDirectories?: boolean; delay?: number }) {
        const includeDirectories = options?.includeDirectories ?? false
        // 已到刷新时机时，仅用 burstDelay 合并「几乎同时到达」的多个完成事件
        const burstDelay = options?.delay ?? 250

        scheduledRefreshIncludesDirectories = scheduledRefreshIncludesDirectories || includeDirectories

        // 已有待触发的刷新：合并本次请求即可，不再叠加定时器（节流核心）
        if (refreshTimer) {
            return
        }

        const sinceLast = Date.now() - lastRefreshAt
        // 距上次刷新已超过最小间隔 → 只等 burstDelay；否则推迟到间隔末尾，
        // 保证整库刷新最多每 REFRESH_MIN_INTERVAL 触发一次。
        const wait = sinceLast >= REFRESH_MIN_INTERVAL ? burstDelay : REFRESH_MIN_INTERVAL - sinceLast

        refreshTimer = setTimeout(() => {
            const shouldRefreshDirectories = scheduledRefreshIncludesDirectories
            scheduledRefreshIncludesDirectories = false
            refreshTimer = null
            lastRefreshAt = Date.now()

            void refreshLibrary(shouldRefreshDirectories)
        }, wait)
    }

    // ============ Directory Actions ============
    async function fetchDirectories() {
        try {
            const { getDirectories } = await import('@/lib/tauri')
            directories.value = await getDirectories()
        } catch (e) {
            console.error('Failed to fetch directories:', e)
        }
    }

    async function addDirectory(path: string) {
        // 检查路径是否已存在（不区分大小写）
        const exists = directories.value.some(d => d.path.toLowerCase() === path.toLowerCase())
        if (exists) {
            throw new Error('Directory already exists')
        }

        try {
            // 先添加目录记录到数据库
            const { addDirectory: addDirectoryCmd } = await import('@/lib/tauri')
            await addDirectoryCmd(path)
            
            // 刷新目录列表以显示新添加的目录
            await fetchDirectories()
            
            // 再调用扫描入库
            const { scanDirectory } = await import('@/lib/tauri')
            const summary = await scanDirectory(path)

            // 扫描完成后刷新视频列表
            await fetchVideos()
            // 再次刷新目录列表以更新统计信息
            await fetchDirectories()

            return summary
        } catch (e) {
            console.error('Failed to scan directory:', e)
            throw e
        }
    }

    async function removeDirectory(id: string) {
        // id 实际上就是 path
        try {
            const { deleteDirectory } = await import('@/lib/tauri')
            await deleteDirectory(id)

            // 刷新数据
            await fetchVideos()
            await fetchDirectories()
        } catch (e) {
            console.error('Failed to remove directory:', e)
            throw e
        }
    }

    async function syncDirectoryCount(id: string) {
        const directory = directories.value.find(d => d.id === id)
        if (!directory) return

        try {
            const { scanDirectory } = await import('@/lib/tauri')
            const summary = await scanDirectory(directory.path)
            directory.videoCount = summary.success_count
            directory.updatedAt = new Date().toISOString()
            
            // 扫描后必须重新获取所有视频，因为可能有新视频加入
            await fetchVideos()

            return summary
        } catch (e) {
            console.error('Failed to sync directory:', e)
            throw e
        }
    }

    async function syncDirectoryCountBatch(ids: string[]) {
        const dirs = ids.map(id => directories.value.find(d => d.id === id)).filter(Boolean)
        if (dirs.length === 0) {
            return {
                success_count: 0,
                failed_count: 0,
                failed_directories: 0,
            }
        }

        const { scanDirectory } = await import('@/lib/tauri')
        const aggregate = {
            success_count: 0,
            failed_count: 0,
            failed_directories: 0,
        }

        // 并发扫描所有目录
        await Promise.allSettled(dirs.map(async (dir) => {
            try {
                const summary = await scanDirectory(dir!.path)
                aggregate.success_count += summary.success_count
                aggregate.failed_count += summary.failed_count
                dir!.videoCount = summary.success_count
                dir!.updatedAt = new Date().toISOString()
            } catch (e) {
                aggregate.failed_directories += 1
                console.error(`Failed to sync directory ${dir!.path}:`, e)
            }
        }))

        // 所有扫描完成后只刷新一次视频列表
        await fetchVideos()

        return aggregate
    }

    return {
        // State
        videos,
        publishers,
        tags,
        directories,
        filter,
        loading,
        error,
        selectedIds,
        coverVersions,
        // Getters
        filteredVideos,
        totalCount,
        filteredCount,
        selectedVideos,
        // Actions
        fetchVideos,
        setFilter,
        resetFilter,
        selectVideo,
        deselectVideo,
        toggleSelect,
        selectAll,
        deselectAll,
        updateVideo,
        removeVideo,
        bumpCoverVersion,
        refreshLibrary,
        scheduleRefresh,
        // Directory Actions
        fetchDirectories,
        addDirectory,
        removeDirectory,
        syncDirectoryCount,
        syncDirectoryCountBatch,
    }
})
