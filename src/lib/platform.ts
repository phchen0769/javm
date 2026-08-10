/**
 * 平台检测工具
 *
 * macOS 使用系统原生窗口交通灯（左上角），其他平台使用自绘窗口按钮（右上角）。
 * 通过 navigator 检测，避免额外引入 @tauri-apps/plugin-os 依赖。
 */
export const isMacOS: boolean =
  typeof navigator !== 'undefined' &&
  (/Mac/i.test(navigator.platform) || /Mac OS X/i.test(navigator.userAgent))
