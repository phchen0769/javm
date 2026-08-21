// 刮削源诊断的展示辅助：状态文案与徽标配色（详情页面板、批量页展开共用）

/** 诊断状态 → 中文文案 */
export function diagStatusText(status: string): string {
  return ({ success: '成功', empty: '无数据', failed: '失败', timeout: '超时' } as Record<string, string>)[status] || status
}

/** 诊断状态 → 徽标配色类 */
export function diagBadgeClass(status: string): string {
  return ({
    success: 'bg-emerald-500/15 text-emerald-600 dark:text-emerald-400',
    empty: 'bg-muted text-muted-foreground',
    failed: 'bg-red-500/15 text-red-600 dark:text-red-400',
    timeout: 'bg-amber-500/15 text-amber-600 dark:text-amber-400',
  } as Record<string, string>)[status] || 'bg-muted text-muted-foreground'
}
