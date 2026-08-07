# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目简介
JAVManager 是一个基于 Tauri 2.0 的桌面视频资源管理工具，核心功能：本地媒体库管理、资源刮削、下载管理、深度链接。

## 技术栈
- **前端**：Vite + Vue 3 + TypeScript + Tailwind CSS + Reka UI
- **后端**：Rust + Tauri 2.0 + rusqlite
- **包管理**：bun（**禁止 npm/yarn/pnpm**）

## 常用命令

**开发与构建**
```bash
bun install                    # 安装依赖
bun run dev                    # 前端开发服务器（仅前端，不含 Tauri）
bun run tauri dev              # 完整桌面应用开发（前端 + Rust 后端）
bun run build                  # 前端构建（含 TypeScript 类型检查）
cargo check                    # Rust 编译检查
```

**测试**（vitest，测试文件命名 `*.spec.ts`，环境为 happy-dom）
```bash
bunx vitest                    # watch 模式运行所有测试
bunx vitest run                # 单次运行所有测试
bunx vitest run src/utils/__tests__/pickRealLink.spec.ts  # 运行单个测试文件
```

**版本管理**
```bash
bun run vb -- patch            # 升级 patch 版本（如 0.5.4 → 0.5.5）
bun run vb -- minor            # 升级 minor 版本（如 0.5.4 → 0.6.0）
bun run vb -- major            # 升级 major 版本（如 0.5.4 → 1.0.0）
bun run vb -- 1.2.3-beta.1     # 指定完整版本号
```
**禁止手动编辑** `package.json`、`tauri.conf.json`、`Cargo.toml` 中的版本号。

## 架构要点

### 前后端通信
- **唯一通道**：Tauri IPC（前端 `invoke` ↔ 后端 `#[tauri::command]`）
- 前端不能直接操作文件系统或数据库，必须通过后端 command
- 新增 command 后必须在 [src-tauri/src/lib.rs](src-tauri/src/lib.rs) 的 `invoke_handler!` 中注册

### 前端目录职责（src/）
- `views/` — 页面级组件，对应路由
- `stores/` — Pinia 状态管理，负责调用 `invoke` 和管理状态
- `components/` — 可复用 UI 组件
- `composables/` — 可复用逻辑组合函数
- `lib/tauri.ts` — 封装 Tauri `invoke` 调用
- `types/` — TypeScript 类型定义
- `utils/` — 纯工具函数

### 后端模块职责（src-tauri/src/）
- `db/` — 数据库访问层（rusqlite + SQLite），带自动迁移逻辑
- `video/` — 视频管理、目录管理、重复检测
- `download/` — 下载任务队列与并发控制（N_m3u8DL-RE 下载器）
- `resource_scrape/` — 资源刮削、站点适配、任务队列
- `scanner/` — 文件扫描与入库
- `media/` — 媒体文件处理、截图、封面
- `metatube/` — MetaTube sidecar 管理（聚合刮削源）
- `actor/` — 演员信息抓取
- `deep_link.rs` — 深度链接解析（`javm://download?url=...`）
- `settings/` — 设置管理
- `analytics.rs` — 匿名统计上报
- `error.rs` — 统一错误类型

### 核心子系统

**数据库迁移**
- 位置：[src-tauri/src/db/mod.rs](src-tauri/src/db/mod.rs)
- 使用 `user_version` pragma 追踪 schema 版本
- 版本落后时自动删除旧库并重建（`check_and_reset_if_needed`）
- WAL 模式在 `init()` 中设置一次，对所有连接生效

**下载管理器**
- 位置：[src-tauri/src/download/](src-tauri/src/download/)
- 并发控制：最大同时下载数可配置（默认 3）
- 状态码（`download/commands.rs`）：0 排队中、1 准备中、2 下载中、3 合并中、4 刮削中、5 已暂停、6 已完成、7 失败、8 重试中、9 已取消
- 应用退出时自动停止进行中的任务
- 下载器二进制位于 `src-tauri/bin/`，跨平台命名规则见 [DEV.md](DEV.md)

**资源刮削队列**
- 位置：[src-tauri/src/resource_scrape/](src-tauri/src/resource_scrape/)
- 支持多站点适配（`sources/` 目录）
- 队列管理器（`queue_manager.rs`）控制并发与重试
- WebView 池（`fetcher.rs`）用于 JS 渲染站点
- 反爬策略（`anti_block/`）：指纹客户端、CF 检测规避

**深度链接**
- 协议：`javm://download?url=<...>&title=<...>`
- 解析逻辑：[src-tauri/src/deep_link.rs](src-tauri/src/deep_link.rs)
- 注册时机：setup 阶段（Linux/Windows debug）、首次启动（macOS）
- 唤起后自动入队下载任务

## 关键约束

1. **包管理器**：只用 `bun`，禁止 npm/yarn/pnpm
2. **语言**：所有对话、注释、commit message 使用中文
3. **Git commit 格式**：`<类型>: <描述>`（类型：feat/fix/refactor/chore 等）
4. **前后端分离**：不能跨越 `src/` 和 `src-tauri/src/` 直接引用
5. **先理解再动手**：修改文件前先阅读，不确定时先搜索源码确认
6. **最小化变更**：只改用户要求的内容，不顺手重构或添加无关功能

## 详细规则
- 编码规范、架构约束：[.claude/rules/architecture.md](.claude/rules/architecture.md)
- Git 提交规则：[.claude/rules/git.md](.claude/rules/git.md)
- 通用行为规则：[.claude/rules/global.md](.claude/rules/global.md)
- 开发事项：[DEV.md](DEV.md)

## Slash 命令
- `/version-release` — 版本升级与发布流程（调用 `bun run vb`，生成发布日志，打 tag，推送远程）
