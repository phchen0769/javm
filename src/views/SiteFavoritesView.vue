<script setup lang="ts">
import { ref, computed } from 'vue'
import { Search, ExternalLink, Globe } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'
import { openUrl } from '@tauri-apps/plugin-opener'

// 网站收藏列表（纯静态数据，来源：blocked-sites-in-south-korea）
const rawSites = import.meta.glob('@/assets/data/sites.txt', { eager: true, query: '?raw', import: 'default' }) as Record<string, string>
const siteText = rawSites[Object.keys(rawSites)[0]] ?? ''

// 解析：按行切分，去空行、去重复，得到去重后的域名列表
const sites = computed<string[]>(() => {
  const seen = new Set<string>()
  const result: string[] = []
  for (const line of siteText.split('\n')) {
    const domain = line.trim()
    if (!domain) continue
    const key = domain.toLowerCase()
    if (seen.has(key)) continue
    seen.add(key)
    result.push(domain)
  }
  return result
})

// 搜索过滤
const keyword = ref('')
const filteredSites = computed(() => {
  const q = keyword.value.trim().toLowerCase()
  if (!q) return sites.value
  return sites.value.filter((s) => s.toLowerCase().includes(q))
})

// 打开收藏站点（首页），用系统默认浏览器
const handleOpen = async (domain: string) => {
  try {
    await openUrl(`https://${domain}`)
  } catch (e) {
    console.error('打开站点失败:', e)
  }
}
</script>

<template>
  <div class="flex h-full flex-col overflow-hidden">
    <!-- 工具栏：搜索框 + 统计 -->
    <div class="flex shrink-0 items-center gap-2 border-b p-4">
      <div class="relative flex-1 max-w-sm">
        <Search class="absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          v-model="keyword"
          placeholder="搜索收藏的网站..."
          class="pl-8"
        />
      </div>
      <span class="text-sm text-muted-foreground">共 {{ filteredSites.length }} 个网站</span>
    </div>

    <!-- 收藏列表 -->
    <ScrollArea class="flex-1">
      <div class="grid grid-cols-1 gap-2 p-4 md:grid-cols-2 lg:grid-cols-3">
        <div
          v-for="site in filteredSites"
          :key="site"
          class="group flex items-center justify-between rounded-lg border p-3 transition-colors hover:border-primary/50 hover:bg-accent/50"
        >
          <div class="flex min-w-0 items-center gap-2">
            <Globe class="size-4 shrink-0 text-muted-foreground" />
            <div class="min-w-0">
              <div class="truncate font-medium">{{ site }}</div>
              <div class="truncate text-xs text-muted-foreground">{{ `https://${site}` }}</div>
            </div>
          </div>
          <Button
            variant="ghost"
            size="sm"
            class="shrink-0"
            @click="handleOpen(site)"
          >
            <ExternalLink class="mr-1 size-3.5" />
            打开
          </Button>
        </div>
      </div>
      <div v-if="filteredSites.length === 0" class="p-8 text-center text-sm text-muted-foreground">
        没有匹配的网站
      </div>
    </ScrollArea>
  </div>
</template>
