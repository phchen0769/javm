<script setup lang="ts">
import { ref, onMounted, onActivated, computed, watch, nextTick } from 'vue'
import { useDebounceFn } from '@vueuse/core'
import { Search, ArrowUpDown, Filter, X, LayoutGrid, List, RefreshCw, RectangleHorizontal, RectangleVertical, LayoutDashboard, Activity, ChevronLeft, ChevronRight } from 'lucide-vue-next'
import { useVideoStore, useSettingsStore } from '@/stores'
import { toast } from 'vue-sonner'
import LibraryHealthDialog from '@/components/LibraryHealthDialog.vue'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Label } from '@/components/ui/label'
import { Separator } from '@/components/ui/separator'
import { Checkbox } from '@/components/ui/checkbox'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import VirtualGrid from '@/components/VirtualGrid.vue'
import VideoDetailDialog from '@/components/VideoDetailDialog.vue'
import ScrapeDialog from '@/components/ScrapeDialog.vue'
import type { Video, ViewMode, CoverType } from '@/types'
import { backfillCoverDimensions, backfillCoverThumbnails, backfillSubtitleFlags, backfillFileCtimes } from '@/lib/tauri'

const ALL_DIRECTORY_VALUE = '__all__'

const videoStore = useVideoStore()
const settingsStore = useSettingsStore()

const searchQuery = ref('')
const detailDialogOpen = ref(false)
const scrapeDialogRef = ref<InstanceType<typeof ScrapeDialog> | null>(null)
const virtualGridRef = ref<InstanceType<typeof VirtualGrid> | null>(null)
const selectedVideo = ref<Video | null>(null)

// 视图模式 - 从设置中读取
const viewMode = computed(() => settingsStore.settings.general.viewMode || 'card')

// 循环切换：卡片 → 瀑布流 → 列表 → 卡片
const VIEW_MODE_CYCLE: ViewMode[] = ['card', 'waterfall', 'list']
const nextViewMode = computed<ViewMode>(() => {
  const idx = VIEW_MODE_CYCLE.indexOf(viewMode.value)
  return VIEW_MODE_CYCLE[(idx + 1) % VIEW_MODE_CYCLE.length]
})
const VIEW_MODE_LABEL: Record<ViewMode, string> = {
  card: '卡片模式',
  waterfall: '瀑布流',
  list: '列表模式',
}
const toggleViewMode = () => {
  settingsStore.updateSettings({ general: { ...settingsStore.settings.general, viewMode: nextViewMode.value } })
}

// 封面类型（横屏/竖屏） - 从设置中读取
const coverType = computed(() => settingsStore.settings.general.coverType || 'landscape')

const toggleCoverType = () => {
  const newType: CoverType = coverType.value === 'landscape' ? 'portrait' : 'landscape'
  settingsStore.updateSettings({ general: { ...settingsStore.settings.general, coverType: newType } })
}

const refreshMediaLibrary = async () => {
  await videoStore.fetchVideos()
  // 拉取失败（含超时）时明确提示，而不是按钮转一圈后悄无声息
  if (videoStore.error) {
    toast.error('刷新失败', { description: videoStore.error })
    return
  }
  virtualGridRef.value?.refreshLayout()
}

// 输入法组合状态
const isComposing = ref(false)

const handleVideoSelect = (video: Video) => {
  selectedVideo.value = video
  detailDialogOpen.value = true
}

const handleVideoUpdated = (video: Video) => {
  selectedVideo.value = video
}

const handleScrape = (video: Video) => {
  if (scrapeDialogRef.value) {
    scrapeDialogRef.value.open(video)
  }
}

// 本地状态管理，用于 UI 绑定
const activeSortBy = ref('title')
const activeSortOrder = ref('desc')

const getFileCreatedAfter = (range?: string) => {
  if (!range) {
    return undefined
  }

  const now = new Date()

  if (range === 'today') {
    const startOfDay = new Date(now)
    startOfDay.setHours(0, 0, 0, 0)
    return startOfDay.toISOString()
  }

  const days = Number.parseInt(range, 10)
  if (Number.isNaN(days)) {
    return undefined
  }

  const threshold = new Date(now.getTime() - days * 24 * 60 * 60 * 1000)
  return threshold.toISOString()
}

const filterState = ref({
  directoryPath: undefined as string | undefined,
  minRating: undefined as string | undefined,
  maxRating: undefined as string | undefined, // Not explicitly requested but good for range
  fileCreatedRange: undefined as string | undefined,
  resolution: [] as string[],
  scraped: [] as string[], // 刮削状态筛选：'scraped' 已刮削, 'unscraped' 未刮削
  censorship: [] as string[], // 有码无码筛选：'censored' 有码, 'uncensored' 无码
  libraryType: [] as string[], // 库类型筛选：'standard' 标准库, 'nonStandard' 非标准库
})

const availableDirectories = computed(() => {
  return [...videoStore.directories].sort((a, b) => a.path.localeCompare(b.path, 'zh-CN'))
})

// 监听排序变化并应用到 Store
watch([activeSortBy, activeSortOrder], ([newBy, newOrder]) => {
  videoStore.setFilter({
    sortBy: newBy as any,
    sortOrder: newOrder as any
  })
})

// 应用筛选
const applyFilters = () => {
  videoStore.setFilter({
    directoryPath: filterState.value.directoryPath,
    minRating: filterState.value.minRating ? parseFloat(filterState.value.minRating) : undefined,
    fileCreatedAfter: getFileCreatedAfter(filterState.value.fileCreatedRange),
    resolution: filterState.value.resolution.length > 0 ? filterState.value.resolution : undefined,
    scraped: filterState.value.scraped.length > 0 ? filterState.value.scraped : undefined,
    censorship: filterState.value.censorship.length > 0 ? filterState.value.censorship : undefined,
    libraryType: filterState.value.libraryType.length > 0 ? filterState.value.libraryType : undefined,
  })
}

// 监听筛选变化自动应用 (或者也可以加 '应用' 按钮，这里选择自动)
watch(filterState, () => {
  applyFilters()
}, { deep: true })

// 搜索：防抖，避免每次按键都触发整库 filter + sort 全量重算
const applySearch = useDebounceFn(() => {
  videoStore.setFilter({ search: searchQuery.value })
}, 250)

const handleSearch = () => {
  // 如果正在输入法组合中，不触发搜索
  if (isComposing.value) {
    return
  }
  applySearch()
}

// 输入法组合开始
const handleCompositionStart = () => {
  isComposing.value = true
}

// 输入法组合结束
const handleCompositionEnd = () => {
  isComposing.value = false
  // 组合结束后立即触发搜索
  handleSearch()
}

// 清除搜索
const clearSearch = () => {
  searchQuery.value = ''
  videoStore.setFilter({ search: '' })
}

onMounted(async () => {
  await videoStore.fetchVideos()
  videoStore.fetchDirectories()
  // 旧库一次性回填文件创建时间（列表排序不再实时 stat），有补写则刷新列表恢复时间顺序
  try {
    const updated = await backfillFileCtimes()
    if (updated > 0) {
      await videoStore.fetchVideos()
    }
  } catch (e) {
    console.error('回填文件创建时间失败:', e)
  }
  // 旧库一次性回填字幕标记（列表不再实时探测文件系统），有补写则刷新列表显示徽标
  try {
    const updated = await backfillSubtitleFlags()
    if (updated > 0) {
      await videoStore.fetchVideos()
    }
  } catch (e) {
    console.error('回填字幕标记失败:', e)
  }
  // 回填存量视频封面尺寸（瀑布流布局需要），有补算则刷新列表拿到尺寸
  try {
    const updated = await backfillCoverDimensions()
    if (updated > 0) {
      await videoStore.fetchVideos()
      virtualGridRef.value?.refreshLayout()
    }
  } catch (e) {
    console.error('回填封面尺寸失败:', e)
  }
  // 回填存量视频网格缩略图（降低网格解码开销、缓解卡顿），有生成则刷新列表拿到缩略图路径
  try {
    const updated = await backfillCoverThumbnails()
    if (updated > 0) {
      await videoStore.fetchVideos()
    }
  } catch (e) {
    console.error('回填网格缩略图失败:', e)
  }
})

onActivated(() => {
  virtualGridRef.value?.refreshLayout()
})

// 为了演示，计算属性直接从 Store 取
const filteredVideos = computed(() => videoStore.filteredVideos)
const hasFilteredResults = computed(() => filteredVideos.value.length > 0)
const isFilteredEmpty = computed(() => !videoStore.loading && !hasFilteredResults.value && videoStore.totalCount > 0)

// 浏览模式（无限滚动 / 上下翻页）与每页条数均从设置读取。
// 两种模式的切片策略不同：
// - 无限滚动：累积切片（slice(0, currentPage*size)），触底只增不减，已渲染行 key 不变，
//   VirtualGrid 不重建 DOM/不重新解码封面，滚动流畅。
// - 上下翻页：替换切片（slice((currentPage-1)*size, ...)），每页独立，跳页后滚回顶部，
//   让「翻页」有明确的内容切换（这正是传统分页的预期）。
const paginationMode = computed(() => settingsStore.settings.general.mediaPagination || 'infinite')
const pageSize = computed(() => settingsStore.settings.general.mediaPageSize || 100)
const currentPage = ref(1)
const totalPages = computed(() => Math.max(1, Math.ceil(filteredVideos.value.length / pageSize.value)))
const displayVideos = computed(() => {
  if (paginationMode.value === 'paged') {
    const start = (currentPage.value - 1) * pageSize.value
    return filteredVideos.value.slice(start, start + pageSize.value)
  }
  return filteredVideos.value.slice(0, currentPage.value * pageSize.value)
})

// 触底自动加载：仅在无限滚动模式下生效（翻页模式靠按钮，触底不自动加载）
const loadMore = () => {
  if (paginationMode.value !== 'infinite') return
  if (currentPage.value < totalPages.value) {
    currentPage.value += 1
  }
}

// 跳转到指定页（翻页模式）：滚回顶部，展示该页内容
const goToPage = (page: number) => {
  const target = Math.min(Math.max(1, page), totalPages.value)
  currentPage.value = target
  nextTick(() => virtualGridRef.value?.scrollToTop())
}
// 下一页
const nextPage = () => {
  if (currentPage.value < totalPages.value) {
    goToPage(currentPage.value + 1)
  }
}
// 上一页
const prevPage = () => {
  if (currentPage.value > 1) {
    goToPage(currentPage.value - 1)
  }
}

// 页码组合框（翻页模式）：一个框既能下拉选页、也能手动输入页码，回车/失焦跳转
const pageInput = ref('1')
const pagePopoverOpen = ref(false)
watch(currentPage, (p) => { pageInput.value = String(p) })
watch(totalPages, () => { pageInput.value = String(currentPage.value) })
const commitPageInput = () => {
  pagePopoverOpen.value = false
  const n = Math.round(Number(pageInput.value))
  if (!Number.isFinite(n)) {
    pageInput.value = String(currentPage.value)
    return
  }
  goToPage(n)
  // 越界输入被 clamp 后同步回显（避免 currentPage 未变时 watch 不触发、输入框残留非法值）
  pageInput.value = String(currentPage.value)
}

// 页码快速选择下拉：列出全部页码，选中即跳转
const pageOptions = computed(() =>
  Array.from({ length: totalPages.value }, (_, i) => ({
    label: `第 ${i + 1} 页`,
    value: String(i + 1),
  })),
)
// 下拉选中某页：跳转并收起
const selectPage = (value: string) => {
  goToPage(Number(value))
  pagePopoverOpen.value = false
}

// 筛选/排序变化时回到第一页（此时才是真正的「重定位」：清空累积、滚回顶部）
watch(() => videoStore.filter, () => {
  currentPage.value = 1
  nextTick(() => virtualGridRef.value?.scrollToTop())
}, { deep: true })
// 列表数据整体刷新后（刮削完成/删除等），当前页超出范围时回退到最后一页
watch(filteredVideos, (list) => {
  const total = Math.max(1, Math.ceil(list.length / pageSize.value))
  if (currentPage.value > total) currentPage.value = total
})
// 每页条数或浏览模式变化时，重置到第一页并回顶（切片方式改变，旧累积位置无意义）
watch([pageSize, paginationMode], () => {
  currentPage.value = 1
  pageInput.value = '1'
  nextTick(() => virtualGridRef.value?.scrollToTop())
})

// 视频总数显示
const videoCount = computed(() => `共 ${filteredVideos.value.length} 个视频`)

// 库健康诊断对话框
const libraryHealthOpen = ref(false)

// 用于重置筛选
const clearFilters = () => {
  filterState.value = {
    directoryPath: undefined,
    minRating: undefined,
    maxRating: undefined,
    fileCreatedRange: undefined,
    resolution: [],
    scraped: [],
    censorship: [],
    libraryType: [],
  }
}

const clearMediaFilters = () => {
  clearSearch()
  clearFilters()
}

// 筛选徽章计数
const activeFilterCount = computed(() => {
  let count = 0
  if (filterState.value.directoryPath) count++
  if (filterState.value.minRating) count++
  if (filterState.value.fileCreatedRange) count++
  if (filterState.value.resolution.length > 0) count++
  if (filterState.value.scraped.length > 0) count++
  if (filterState.value.censorship.length > 0) count++
  if (filterState.value.libraryType.length > 0) count++
  return count
})

// 分辨率 checkbox 计算属性
const resolution4K = computed({
  get: () => filterState.value.resolution.includes('4K'),
  set: (val) => {
    if (val) {
      filterState.value.resolution = [...filterState.value.resolution, '4K']
    } else {
      filterState.value.resolution = filterState.value.resolution.filter(i => i !== '4K')
    }
  }
})

const resolution1080p = computed({
  get: () => filterState.value.resolution.includes('1080p'),
  set: (val) => {
    if (val) {
      filterState.value.resolution = [...filterState.value.resolution, '1080p']
    } else {
      filterState.value.resolution = filterState.value.resolution.filter(i => i !== '1080p')
    }
  }
})

const resolution720p = computed({
  get: () => filterState.value.resolution.includes('720p'),
  set: (val) => {
    if (val) {
      filterState.value.resolution = [...filterState.value.resolution, '720p']
    } else {
      filterState.value.resolution = filterState.value.resolution.filter(i => i !== '720p')
    }
  }
})

const resolutionSD = computed({
  get: () => filterState.value.resolution.includes('SD'),
  set: (val) => {
    if (val) {
      filterState.value.resolution = [...filterState.value.resolution, 'SD']
    } else {
      filterState.value.resolution = filterState.value.resolution.filter(i => i !== 'SD')
    }
  }
})

// 刮削状态 checkbox 计算属性
const scrapedChecked = computed({
  get: () => filterState.value.scraped.includes('scraped'),
  set: (val) => {
    if (val) {
      filterState.value.scraped = [...filterState.value.scraped, 'scraped']
    } else {
      filterState.value.scraped = filterState.value.scraped.filter(i => i !== 'scraped')
    }
  }
})

const unscrapedChecked = computed({
  get: () => filterState.value.scraped.includes('unscraped'),
  set: (val) => {
    if (val) {
      filterState.value.scraped = [...filterState.value.scraped, 'unscraped']
    } else {
      filterState.value.scraped = filterState.value.scraped.filter(i => i !== 'unscraped')
    }
  }
})

// 有码/无码 checkbox 计算属性
const censoredChecked = computed({
  get: () => filterState.value.censorship.includes('censored'),
  set: (val) => {
    if (val) {
      filterState.value.censorship = [...filterState.value.censorship, 'censored']
    } else {
      filterState.value.censorship = filterState.value.censorship.filter(i => i !== 'censored')
    }
  }
})

const uncensoredChecked = computed({
  get: () => filterState.value.censorship.includes('uncensored'),
  set: (val) => {
    if (val) {
      filterState.value.censorship = [...filterState.value.censorship, 'uncensored']
    } else {
      filterState.value.censorship = filterState.value.censorship.filter(i => i !== 'uncensored')
    }
  }
})

// 标准库/非标准库 checkbox 计算属性
const standardChecked = computed({
  get: () => filterState.value.libraryType.includes('standard'),
  set: (val) => {
    if (val) {
      filterState.value.libraryType = [...filterState.value.libraryType, 'standard']
    } else {
      filterState.value.libraryType = filterState.value.libraryType.filter(i => i !== 'standard')
    }
  }
})

const nonStandardChecked = computed({
  get: () => filterState.value.libraryType.includes('nonStandard'),
  set: (val) => {
    if (val) {
      filterState.value.libraryType = [...filterState.value.libraryType, 'nonStandard']
    } else {
      filterState.value.libraryType = filterState.value.libraryType.filter(i => i !== 'nonStandard')
    }
  }
})

// 从库健康面板跳转：仅显示非标准库(无番号)
const showNonStandardLibrary = () => {
  filterState.value.libraryType = ['nonStandard']
  libraryHealthOpen.value = false
}

</script>

<template>
  <div class="flex h-full flex-col">
    <!-- 工具栏 -->
    <div class="flex items-center gap-2 border-b p-4">
      <!-- 搜索框 -->
      <div class="relative mr-2" style="width: 200px;">
        <Input 
          v-model="searchQuery" 
          placeholder="搜索" 
          class="pr-16 h-9" 
          @input="handleSearch"
          @compositionstart="handleCompositionStart"
          @compositionend="handleCompositionEnd"
        />
        <div class="absolute right-1 top-1/2 -translate-y-1/2 flex items-center gap-1">
          <button
            v-if="searchQuery"
            @click="clearSearch"
            class="text-muted-foreground hover:text-foreground transition-colors"
            type="button"
          >
            <X class="size-4" />
          </button>
          <button
            @click="handleSearch"
            class="text-muted-foreground hover:text-foreground transition-colors p-1 rounded-sm hover:bg-accent"
            type="button"
          >
            <Search class="size-4" />
          </button>
        </div>
      </div>

      <!-- 排序下拉菜单 -->
      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <Button variant="outline" size="sm" class="h-9 gap-1">
            <ArrowUpDown class="size-4 text-muted-foreground" />
            排序
            <Badge v-if="activeSortBy !== 'title'" variant="secondary" class="ml-1 h-5 px-1 text-[10px]">
              {{ activeSortBy === 'premiered' ? '发行日期' : activeSortBy === 'fileCreatedAt' ? '文件创建时间' : activeSortBy === 'duration' ? '时长' : activeSortBy === 'rating' ? '评分' : activeSortBy === 'fileSize' ? '大小' : '自定义' }}
            </Badge>
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" class="w-48">
          <DropdownMenuLabel>排序依据</DropdownMenuLabel>
          <DropdownMenuRadioGroup v-model="activeSortBy">
            <DropdownMenuRadioItem value="title">名称</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="fileCreatedAt">文件创建时间</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="premiered">发行日期</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="duration">时长</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="rating">评分</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="fileSize">大小</DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>
          <DropdownMenuSeparator />
          <DropdownMenuLabel>顺序</DropdownMenuLabel>
          <DropdownMenuRadioGroup v-model="activeSortOrder">
            <DropdownMenuRadioItem value="desc">降序 (9-0)</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="asc">升序 (0-9)</DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>
        </DropdownMenuContent>
      </DropdownMenu>

      <!-- 复合筛选 Popover -->
      <Popover>
        <PopoverTrigger as-child>
          <Button variant="outline" size="sm" class="h-9 gap-1" :class="activeFilterCount > 0 ? 'bg-secondary/50' : ''">
            <Filter class="size-4 text-muted-foreground" />
            筛选
            <Badge v-if="activeFilterCount > 0" variant="default"
              class="ml-1 h-5 w-5 p-0 flex items-center justify-center rounded-full text-[10px]">
              {{ activeFilterCount }}
            </Badge>
          </Button>
        </PopoverTrigger>
        <PopoverContent class="w-80 p-4" align="start">
          <div class="space-y-4">
            <div class="flex items-center justify-between">
              <h4 class="font-medium leading-none">筛选条件</h4>
              <Button variant="ghost" size="sm" class="h-auto p-0 text-muted-foreground" @click="clearFilters">
                清空
              </Button>
            </div>
            <Separator />

            <div class="space-y-2">
              <Label class="text-xs text-muted-foreground">目录</Label>
              <Select :model-value="filterState.directoryPath ?? ALL_DIRECTORY_VALUE"
                @update:model-value="(v) => { filterState.directoryPath = String(v) === ALL_DIRECTORY_VALUE ? undefined : String(v) }">
                <SelectTrigger class="h-8">
                  <SelectValue placeholder="全部目录" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem :value="ALL_DIRECTORY_VALUE">全部目录</SelectItem>
                  <SelectItem v-for="directory in availableDirectories" :key="directory.id" :value="directory.path">
                    {{ directory.path }}
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>

            <Separator />

            <!-- 评分筛选 -->
            <div class="space-y-2">
              <Label class="text-xs text-muted-foreground">最低评分</Label>
              <Select v-model="filterState.minRating">
                <SelectTrigger class="h-8">
                  <SelectValue placeholder="不限" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="0">0 分</SelectItem>
                  <SelectItem value="1">1 分</SelectItem>
                  <SelectItem value="2">2 分</SelectItem>
                  <SelectItem value="3">3 分</SelectItem>
                  <SelectItem value="4">4 分</SelectItem>
                  <SelectItem value="5">5 分</SelectItem>
                  <SelectItem value="6">6 分</SelectItem>
                  <SelectItem value="7">7 分</SelectItem>
                  <SelectItem value="8">8 分</SelectItem>
                  <SelectItem value="9">9 分</SelectItem>
                  <SelectItem value="10">10 分</SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div class="space-y-2">
              <Label class="text-xs text-muted-foreground">文件创建时间</Label>
              <Select v-model="filterState.fileCreatedRange">
                <SelectTrigger class="h-8">
                  <SelectValue placeholder="不限" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="today">今天</SelectItem>
                  <SelectItem value="1">最近 24 小时</SelectItem>
                  <SelectItem value="3">最近 3 天</SelectItem>
                  <SelectItem value="7">最近 7 天</SelectItem>
                  <SelectItem value="30">最近 30 天</SelectItem>
                </SelectContent>
              </Select>
            </div>

            <!-- 分辨率筛选 -->
            <div class="space-y-2">
              <Label class="text-xs text-muted-foreground">分辨率</Label>
              <div class="grid grid-cols-2 gap-2">
                <div class="flex items-center space-x-2">
                  <Checkbox id="res-4k" v-model="resolution4K" />
                  <label for="res-4k"
                    class="text-sm font-medium leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70 cursor-pointer">4K</label>
                </div>
                <div class="flex items-center space-x-2">
                  <Checkbox id="res-1080p" v-model="resolution1080p" />
                  <label for="res-1080p"
                    class="text-sm font-medium leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70 cursor-pointer">1080p</label>
                </div>
                <div class="flex items-center space-x-2">
                  <Checkbox id="res-720p" v-model="resolution720p" />
                  <label for="res-720p"
                    class="text-sm font-medium leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70 cursor-pointer">720p</label>
                </div>
                <div class="flex items-center space-x-2">
                  <Checkbox id="res-sd" v-model="resolutionSD" />
                  <label for="res-sd"
                    class="text-sm font-medium leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70 cursor-pointer">SD</label>
                </div>
              </div>
            </div>

            <!-- 刮削状态筛选 -->
            <div class="space-y-2">
              <Label class="text-xs text-muted-foreground">刮削状态</Label>
              <div class="grid grid-cols-2 gap-2">
                <div class="flex items-center space-x-2">
                  <Checkbox id="scraped" v-model="scrapedChecked" />
                  <label for="scraped"
                    class="text-sm font-medium leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70 cursor-pointer">已刮削</label>
                </div>
                <div class="flex items-center space-x-2">
                  <Checkbox id="unscraped" v-model="unscrapedChecked" />
                  <label for="unscraped"
                    class="text-sm font-medium leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70 cursor-pointer">未刮削</label>
                </div>
              </div>
            </div>

            <!-- 有码/无码筛选 -->
            <div class="space-y-2">
              <Label class="text-xs text-muted-foreground">有码/无码</Label>
              <div class="grid grid-cols-2 gap-2">
                <div class="flex items-center space-x-2">
                  <Checkbox id="censored" v-model="censoredChecked" />
                  <label for="censored"
                    class="text-sm font-medium leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70 cursor-pointer">有码</label>
                </div>
                <div class="flex items-center space-x-2">
                  <Checkbox id="uncensored" v-model="uncensoredChecked" />
                  <label for="uncensored"
                    class="text-sm font-medium leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70 cursor-pointer">无码</label>
                </div>
              </div>
            </div>

            <!-- 标准库/非标准库筛选 -->
            <div class="space-y-2">
              <Label class="text-xs text-muted-foreground">库类型</Label>
              <div class="grid grid-cols-2 gap-2">
                <div class="flex items-center space-x-2">
                  <Checkbox id="standard" v-model="standardChecked" />
                  <label for="standard"
                    class="text-sm font-medium leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70 cursor-pointer">标准库</label>
                </div>
                <div class="flex items-center space-x-2">
                  <Checkbox id="nonStandard" v-model="nonStandardChecked" />
                  <label for="nonStandard"
                    class="text-sm font-medium leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70 cursor-pointer">非标准库</label>
                </div>
              </div>
            </div>
          </div>
        </PopoverContent>
      </Popover>

      <!-- 库健康诊断 -->
      <Button variant="outline" size="sm" class="h-9 gap-1" @click="libraryHealthOpen = true">
        <Activity class="size-4 text-muted-foreground" />
        库健康
      </Button>

      <div class="ml-auto flex items-center gap-2">
        <!-- 统计信息 -->
        <span class="text-sm text-muted-foreground">{{ videoCount }}</span>

        <!-- 翻页（仅上下翻页模式显示） -->
        <div v-if="paginationMode === 'paged' && totalPages > 1" class="flex items-center gap-1">
          <Button
            variant="ghost"
            size="icon"
            class="h-8 w-8"
            title="上一页"
            :disabled="currentPage <= 1"
            @click="prevPage"
          >
            <ChevronLeft class="size-4" />
          </Button>
          <!-- 页码组合框：既能下拉选页，也能手动输入页码 -->
          <Popover v-model:open="pagePopoverOpen">
            <PopoverTrigger as-child>
              <div class="flex items-center gap-1 text-sm text-muted-foreground tabular-nums">
                <Input
                  type="number"
                  min="1"
                  :max="totalPages"
                  class="h-8 w-12 px-2 text-center"
                  :model-value="pageInput"
                  @update:model-value="(v) => (pageInput = String(v))"
                  @focus="pagePopoverOpen = true"
                  @keydown.enter="commitPageInput"
                  @blur="commitPageInput"
                />
                <span>/ {{ totalPages }}</span>
              </div>
            </PopoverTrigger>
            <PopoverContent class="w-36 p-1" align="start" side="bottom">
              <ScrollArea class="max-h-72">
                <div
                  v-for="opt in pageOptions"
                  :key="opt.value"
                  class="cursor-pointer rounded-md px-2 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground"
                  :class="String(currentPage) === opt.value ? 'bg-accent text-accent-foreground' : ''"
                  @mousedown.prevent="selectPage(opt.value)"
                >
                  {{ opt.label }}
                </div>
              </ScrollArea>
            </PopoverContent>
          </Popover>
          <Button
            variant="ghost"
            size="icon"
            class="h-8 w-8"
            title="下一页"
            :disabled="currentPage >= totalPages"
            @click="nextPage"
          >
            <ChevronRight class="size-4" />
          </Button>
        </div>

        <!-- 视图模式切换 -->
        <Button
          variant="ghost"
          size="icon"
          class="h-9 w-9"
          :title="`切换到${VIEW_MODE_LABEL[nextViewMode]}`"
          @click="toggleViewMode"
        >
          <LayoutDashboard v-if="viewMode === 'waterfall'" class="size-4" />
          <List v-else-if="viewMode === 'list'" class="size-4" />
          <LayoutGrid v-else class="size-4" />
        </Button>

        <!-- 封面横竖屏切换 -->
        <Button
          v-if="viewMode === 'card'"
          variant="ghost"
          size="icon"
          class="h-9 w-9"
          :title="coverType === 'landscape' ? '切换到竖屏封面' : '切换到横屏封面'"
          @click="toggleCoverType"
        >
          <RectangleVertical v-if="coverType === 'landscape'" class="size-4" />
          <RectangleHorizontal v-else class="size-4" />
        </Button>

        <Button
          variant="ghost"
          size="icon"
          class="h-9 w-9"
          title="刷新多媒体页"
          :disabled="videoStore.loading"
          @click="refreshMediaLibrary"
        >
          <RefreshCw class="size-4" :class="{ 'animate-spin': videoStore.loading }" />
        </Button>
      </div>
    </div>

    <!-- 视频网格 -->
    <div class="flex-1 overflow-hidden py-4">
      <div v-if="isFilteredEmpty" class="flex h-full flex-col items-center justify-center gap-4 text-center">
        <div class="space-y-2 text-muted-foreground">
          <p class="text-lg text-foreground">当前筛选条件下暂无视频</p>
          <p class="text-sm">批量刮削结束后，若当前只显示未刮削视频，列表会变为空。清空搜索或筛选后可恢复显示。</p>
        </div>
        <Button variant="outline" @click="clearMediaFilters">
          清空筛选
        </Button>
      </div>
      <VirtualGrid
        ref="virtualGridRef"
        v-else
        :items="displayVideos"
        :loading="videoStore.loading && videoStore.totalCount === 0"
        :view-mode="viewMode"
        @select="handleVideoSelect"
        @scrape="handleScrape"
        @load-more="loadMore"
      />
    </div>

    <!-- 视频详情对话框 -->
    <VideoDetailDialog v-model:open="detailDialogOpen" :video="selectedVideo" @video-updated="handleVideoUpdated" />

    <!-- 刮削对话框 -->
    <ScrapeDialog ref="scrapeDialogRef" @success="videoStore.fetchVideos()" />

    <!-- 库健康诊断 -->
    <LibraryHealthDialog v-model:open="libraryHealthOpen" @view-non-standard="showNonStandardLibrary" />
  </div>
</template>
