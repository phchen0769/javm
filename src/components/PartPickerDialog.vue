<script setup lang="ts">
import { computed } from 'vue'
import { Play } from 'lucide-vue-next'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from '@/components/ui/dialog'
import type { Video, VideoPart } from '@/types'

interface Props {
  open: boolean
  video?: Video | null
}

const props = defineProps<Props>()

const emit = defineEmits<{
  'update:open': [value: boolean]
  // 选中某分段进行播放
  'play': [path: string]
}>()

const isOpen = computed({
  get: () => props.open,
  set: (value) => emit('update:open', value),
})

const title = computed(() => props.video?.title || props.video?.originalTitle || props.video?.localId || '该视频')

// 按段序号排序（后端已排序，这里兜底一次）
const parts = computed<VideoPart[]>(() =>
  [...(props.video?.parts ?? [])].sort((a, b) => (a.partIndex ?? 0) - (b.partIndex ?? 0)),
)

// 文件名（去路径），兼容 Windows / POSIX 分隔符
const fileName = (path: string) => path.split(/[\\/]/).pop() || path

const handlePick = (path: string) => {
  emit('play', path)
  isOpen.value = false
}
</script>

<template>
  <Dialog v-model:open="isOpen">
    <DialogContent class="sm:max-w-[480px]">
      <DialogTitle>选择要播放的分段</DialogTitle>
      <DialogDescription>
        「{{ title }}」共 {{ parts.length }} 段，请选择要用系统播放器打开的分段。
      </DialogDescription>
      <div class="mt-2 flex flex-col gap-1 max-h-[50vh] overflow-y-auto">
        <button
          v-for="part in parts"
          :key="part.videoPath"
          type="button"
          class="flex items-center gap-3 rounded-md px-3 py-2 text-left hover:bg-muted transition-colors"
          @click="handlePick(part.videoPath)"
        >
          <span class="flex size-6 shrink-0 items-center justify-center rounded-full bg-indigo-500/90 text-white text-xs">
            {{ part.partIndex }}
          </span>
          <span class="flex-1 truncate text-sm" :title="part.videoPath">
            {{ fileName(part.videoPath) }}
          </span>
          <Play class="size-4 shrink-0 text-muted-foreground" />
        </button>
      </div>
    </DialogContent>
  </Dialog>
</template>
