// Tauri IPC 传输通道选择（仅 macOS）：改走 postMessage，弃用自定义协议通道。
//
// Tauri 2 默认用 fetch(`ipc://localhost/<cmd>`) 承载 invoke，响应由 wry 在 tokio 工作线程上直接调用
// WKURLSchemeTask 的 didReceiveResponse/didReceiveData/didFinish 写回；而 WebKit 要求这些调用只能在
// 主线程。本机实测（javm.log 里的 [frontend:warn] "Couldn't find callback id"、"[unhandledrejection] null"、
// rs_get_scrape_tasks 以成功数据被 reject）在批量刮削等 IPC 高并发时会出现响应错投或丢失：
// invoke 永不返回 → 媒体库 loading 卡死、刷新按钮永久禁用、刮削后列表不更新。
//
// 处理：让指向 ipc:// 的 fetch 直接失败，触发 Tauri 内置回退（tauri/scripts/ipc-protocol.js：
// 自定义协议 fetch 失败即置 customProtocolIpcFailed 并改用 window.ipc.postMessage，响应经主线程
// eval 回调），之后所有 invoke 都走主线程的 postMessage 通道。
// 注意：升级 Tauri 后须确认 ipc-protocol.js 仍保留该回退逻辑。
import { info } from '@tauri-apps/plugin-log'

const IPC_PROTOCOL_PREFIX = 'ipc://localhost/'

function requestUrl(input: RequestInfo | URL): string {
    if (typeof input === 'string') return input
    if (input instanceof URL) return input.href
    return input.url
}

const isTauri = typeof window !== 'undefined' && Boolean((window as any).__TAURI_INTERNALS__)
const isMac = typeof navigator !== 'undefined' && /Macintosh|Mac OS X/.test(navigator.userAgent)
// 回退通道不存在时不动手，避免把唯一可用的通道也切断
const hasPostMessageIpc = typeof (window as any).ipc?.postMessage === 'function'

if (isTauri && isMac && hasPostMessageIpc) {
    const nativeFetch = window.fetch.bind(window)
    window.fetch = (input, init) => {
        if (requestUrl(input).startsWith(IPC_PROTOCOL_PREFIX)) {
            return Promise.reject(new Error('macOS 下禁用 ipc:// 自定义协议通道，改用 postMessage'))
        }
        return nativeFetch(input, init)
    }
    // 立即发一次 invoke 触发 Tauri 的回退切换（此时 console 尚未接入日志转发，回退提示只进 devtools），
    // 顺带在日志里留下通道标记
    void info('[ipc] macOS 已改用 postMessage 通道')
}
