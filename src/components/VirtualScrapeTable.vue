<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { useVirtualizer } from '@tanstack/vue-virtual'
import { FolderOpen, Play, Square, Trash2, ChevronRight } from 'lucide-vue-next'
import { Badge } from '@/components/ui/badge'
import { Progress } from '@/components/ui/progress'
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from '@/components/ui/context-menu'
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip'
import type { ScrapeTask, SourceDiagnostic } from '@/types'
import { SCRAPE_STATUS_TEXT } from '@/utils/constants'
import { diagStatusText, diagBadgeClass } from '@/utils/scrapeDiagnostic'
import { useResourceScrapeStore } from '@/stores/resourceScrape'

interface Props {
  tasks: ScrapeTask[]
}

const props = defineProps<Props>()

const emit = defineEmits<{
  (e: 'openFolder', task: ScrapeTask): void
  (e: 'startTask', task: ScrapeTask): void
  (e: 'stopTask', task: ScrapeTask): void
  (e: 'removeTask', task: ScrapeTask): void
}>()

const store = useResourceScrapeStore()

// 容器引用
const containerRef = ref<HTMLElement>()

// 折叠时的行高（展开后由 measureElement 动态测量）
const ROW_HEIGHT = 60

// 当前展开查看诊断的任务 id（同时只展开一个，避免多次拉取）
const expandedTaskId = ref<string | null>(null)

/** 展开/收起某任务的各源诊断；展开时按需拉取（完成后才有数据） */
const toggleExpand = async (task: ScrapeTask) => {
  if (expandedTaskId.value === task.id) {
    expandedTaskId.value = null
    return
  }
  expandedTaskId.value = task.id
  try {
    await store.fetchTaskDiagnostics(task.id)
  } catch (e) {
    console.error('获取任务诊断失败:', e)
  }
}

/** 读取某任务已缓存的诊断 */
const diagsFor = (taskId: string): SourceDiagnostic[] => store.taskDiagnostics[taskId] ?? []

// 展开中的任务跑到终态时自动补拉诊断（用户在运行中展开、完成后无需手动重开即可看到结果）
watch(
  () => (expandedTaskId.value ? props.tasks.find(t => t.id === expandedTaskId.value)?.status : null),
  (status) => {
    if (expandedTaskId.value && ['completed', 'failed', 'partial'].includes(status ?? '')) {
      store.fetchTaskDiagnostics(expandedTaskId.value).catch(() => {})
    }
  },
)

// 刮削步骤文本映射
const STEP_TEXT: Record<number, string> = {
  0: '等待中',
  1: '验证 Cloudflare',
  2: '获取元数据',
  3: '下载封面',
  4: '保存 .nfo 文件',
  5: '更新数据库',
}

const getStepText = (progress: number) => {
  return STEP_TEXT[progress] || '未知状态'
}

// 刮削状态颜色
const getStatusVariant = (status: string): 'default' | 'secondary' | 'destructive' | 'outline' => {
  switch (status) {
    case 'running': return 'default'
    case 'completed': return 'secondary'
    case 'failed': return 'destructive'
    default: return 'outline'
  }
}

// 虚拟化器（展开行高度不定，用 measureElement 动态测量）
const virtualizer = useVirtualizer({
  get count() { return props.tasks.length },
  getScrollElement: () => containerRef.value ?? null,
  estimateSize: () => ROW_HEIGHT,
  overscan: 5,
})

// 动态测量：把行元素交给虚拟化器测高（展开/收起、诊断异步加载后经 ResizeObserver 自动重测）
const measureElement = (el: any) => {
  if (el) virtualizer.value.measureElement(el as Element)
}

// 虚拟行（附带对应任务，便于模板取用）
const rows = computed(() =>
  virtualizer.value.getVirtualItems().map((vr) => ({ vr, task: props.tasks[vr.index] })),
)

// 总高度
const totalHeight = computed(() => virtualizer.value.getTotalSize())
</script>

<template>
  <div ref="containerRef" class="flex-1 overflow-auto">
    <div :style="{ height: `${totalHeight}px`, position: 'relative' }">
      <div
        v-for="{ vr, task } in rows"
        :key="String(vr.key)"
        :data-index="vr.index"
        :ref="measureElement"
        :style="{
          position: 'absolute',
          top: 0,
          left: 0,
          width: '100%',
          transform: `translateY(${vr.start}px)`,
        }"
      >
        <ContextMenu>
          <ContextMenuTrigger as-child>
            <div
              class="flex items-center border-b min-h-[60px] hover:bg-muted/50 transition-colors cursor-pointer"
              @click="toggleExpand(task)"
            >
              <!-- 展开指示 -->
              <div class="w-7 shrink-0 flex items-center justify-center text-muted-foreground">
                <ChevronRight
                  class="size-4 transition-transform"
                  :class="{ 'rotate-90': expandedTaskId === task.id }"
                />
              </div>

              <!-- 路径 -->
              <div class="flex-1 min-w-0 px-4 py-3">
                <div class="text-sm truncate" :title="task.path">
                  {{ task.path }}
                </div>
              </div>

              <!-- 进度 -->
              <div class="w-40 shrink-0 px-4 py-3">
                <TooltipProvider>
                  <Tooltip>
                    <TooltipTrigger as-child>
                      <div class="flex items-center gap-2 cursor-help">
                        <Progress
                          :model-value="task.status === 'completed' ? 100 : Math.min(task.progress * 20, 100)"
                          class="h-2 w-full"
                        />
                      </div>
                    </TooltipTrigger>
                    <TooltipContent>
                      <p>{{ getStepText(task.progress) }}</p>
                    </TooltipContent>
                  </Tooltip>
                </TooltipProvider>
              </div>

              <!-- 状态 -->
              <div class="w-24 shrink-0 px-4 py-3">
                <Badge :variant="getStatusVariant(task.status)">
                  {{ SCRAPE_STATUS_TEXT[task.status] }}
                </Badge>
              </div>
            </div>
          </ContextMenuTrigger>

          <ContextMenuContent>
            <ContextMenuItem @click="emit('openFolder', task)">
              <FolderOpen class="mr-2 size-4" />
              打开所在目录
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem
              :disabled="task.status === 'running'"
              @click="emit('startTask', task)"
            >
              <Play class="mr-2 size-4" />
              开始任务
            </ContextMenuItem>
            <ContextMenuItem
              :disabled="task.status !== 'running'"
              @click="emit('stopTask', task)"
            >
              <Square class="mr-2 size-4" />
              停止任务
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem @click="emit('removeTask', task)">
              <Trash2 class="mr-2 size-4" />
              删除任务
            </ContextMenuItem>
          </ContextMenuContent>
        </ContextMenu>

        <!-- 展开：各源刮削诊断（来自哪个网址/是否有效） -->
        <div v-if="expandedTaskId === task.id" class="border-b bg-muted/20 px-4 py-2 text-xs">
          <div v-if="diagsFor(task.id).length > 0" class="space-y-1">
            <div v-for="d in diagsFor(task.id)" :key="d.source" class="flex items-center gap-2">
              <span
                class="inline-flex min-w-[3rem] shrink-0 justify-center rounded px-1.5 py-0.5 text-[10px] font-medium"
                :class="diagBadgeClass(d.status)"
              >
                {{ diagStatusText(d.status) }}
              </span>
              <span class="shrink-0 font-medium">{{ d.siteName }}</span>
              <span v-if="d.url" class="truncate font-mono text-muted-foreground" :title="d.error || d.url">{{ d.url }}</span>
              <span v-else-if="d.error" class="truncate text-muted-foreground" :title="d.error">{{ d.error }}</span>
              <span class="ml-auto shrink-0 tabular-nums text-muted-foreground">{{ d.elapsedMs }}ms</span>
            </div>
          </div>
          <div v-else class="py-1 text-muted-foreground">
            {{ ['completed', 'failed', 'partial'].includes(task.status)
              ? '暂无来源诊断（可能在重启前或旧版本刮削），重跑此任务后可见'
              : '刮削中或尚未开始，完成后可见各源诊断' }}
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* shadcn 风格滚动条 */
.overflow-auto {
  scrollbar-width: thin;
  scrollbar-color: transparent transparent;
}

.overflow-auto:hover {
  scrollbar-color: hsl(0 0% 20%) transparent;
}

.overflow-auto::-webkit-scrollbar {
  width: 10px;
}

.overflow-auto::-webkit-scrollbar-track {
  background: transparent;
}

.overflow-auto::-webkit-scrollbar-thumb {
  background-color: transparent;
  border-radius: 9999px;
  border: 2px solid transparent;
  background-clip: content-box;
  transition: background-color 0.2s;
}

.overflow-auto:hover::-webkit-scrollbar-thumb {
  background-color: hsl(0 0% 20%);
}

.overflow-auto::-webkit-scrollbar-thumb:hover {
  background-color: hsl(0 0% 30%);
}
</style>
