# Gloss 开发文档

本文面向 Gloss 的开发者和维护者。用户安装、使用和故障排查说明请阅读仓库根目录的 [README](../README.md)。

## 产品边界

Gloss 是仅支持 Windows 的轻量阅读 Agent。它读取用户主动选中的文本，通过 ChatGPT Codex Responses 接口执行 Triage、Translate 和围绕原文的多轮对话。

## 技术栈

- 原生壳层：Rust、Tauri 2
- 前端：React 19、TypeScript、Vite
- 文本选区：Windows UI Automation `TextPattern`
- AI 接口：ChatGPT OAuth、Codex Responses endpoint
- 默认模型：`gpt-5.6-luna`
- 会话存储：每个会话一个 JSONL 文件
- 安装包：NSIS x64

## 代码结构

| 路径 | 职责 |
| --- | --- |
| `src/App.tsx` | 浮动工具栏、结果卡片、历史和设置界面 |
| `src/App.css` | 黑白主题、组件和窗口表面样式 |
| `src/markdown.ts` | Markdown 渲染兼容处理 |
| `src-tauri/src/lib.rs` | Tauri 生命周期、命令注册和托盘入口 |
| `src-tauri/src/selection.rs` | Windows UI Automation 选区读取 |
| `src-tauri/src/overlay.rs` | 工具栏与卡片窗口的尺寸和位置 |
| `src-tauri/src/oauth.rs` | ChatGPT OAuth 登录、刷新和本地凭据 |
| `src-tauri/src/network.rs` | HTTP 与 SOCKS 代理配置 |
| `src-tauri/src/responses.rs` | Responses 流式请求、解析与重试 |
| `src-tauri/src/sessions.rs` | 会话模型、请求体和 JSONL 持久化 |
| `src-tauri/src/agent.rs` | Triage、Translate、继续提问和事件流 |
| `src-tauri/src/profile.rs` | CEFR 学习画像更新与合并 |
| `src-tauri/src/prompts.rs` | System Prompt 与运行时上下文模板加载 |
| `src-tauri/prompts/` | 随应用打包的默认 Prompt 模板 |

## 开发环境

需要准备：

- Windows 10 或 Windows 11
- Node.js
- pnpm
- Rust 工具链及 Tauri 2 在 Windows 上需要的构建依赖

安装依赖并启动开发版本：

```powershell
pnpm install
pnpm tauri dev
```

使用内置示例数据启动浏览器预览：

```powershell
pnpm dev
```

## 验证

提交代码前至少运行：

```powershell
pnpm build
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

涉及全局快捷键、UI Automation、透明窗口、拖动区域、托盘或 OAuth 回调的修改，还需要在真实 Windows 桌面环境中手动验证。

## 构建发布版本

```powershell
pnpm tauri build
```

主要产物：

```text
src-tauri\target\release\gloss.exe
src-tauri\target\release\bundle\nsis\Gloss_<version>_x64-setup.exe
```

构建前若 release 目录中的 `gloss.exe` 正在运行，需要先关闭该进程，否则 Windows 会阻止覆盖文件。

## 运行流程

1. 全局快捷键触发选区捕获。
2. `selection.rs` 从当前焦点元素或其祖先读取 UI Automation `TextPattern`。
3. `overlay.rs` 在选区附近显示工具栏；无法取得可靠锚点时使用回退位置。
4. 用户选择 Triage 或 Translate 后，`agent.rs` 创建会话并组装 Responses 请求。
5. `responses.rs` 通过事件流把增量结果发送给 React 界面。
6. `sessions.rs` 将完整会话写入 JSONL。
7. Triage 至少发生一次继续提问后，`profile.rs` 才会在后台评估并更新 CEFR 画像。

## Responses 接口与重试

请求体使用 Responses API 的消息与输出项结构，`store` 固定为 `false`。继续提问时会重放当前会话需要的完整消息，每次请求都能根据本地会话独立完成。

网络传输、暂时性 HTTP 状态或不完整流式响应最多自动重试两次。收到未授权响应时会尝试刷新 OAuth 令牌；仍然失败后，界面会保留可用的部分输出并提供手动重试。

模型和 Prompt 元数据定义在 `src-tauri/src/sessions.rs`：

```rust
pub const MODEL: &str = "gpt-5.6-luna";
pub const PROMPT_VERSION: u32 = 3;
```

## Prompt 模板

打包默认值位于 `src-tauri/prompts/`：

- `system.md`：定义 Gloss 的身份、对话行为和学习画像使用方式
- `triage.md`：首轮 Triage runtime context
- `translate.md`：首轮 Translate runtime context
- `explain-selection.md`：Triage 结果划词后的快捷解释 runtime context
- `learner-profile.md`：根据多轮 Triage 对话生成 CEFR 画像补丁

首次启动时，Gloss 会把缺失的模板复制到：

```text
%APPDATA%\com.gloss.desktop\prompts\
```

每次请求都会重新读取模板。首条用户消息由 action runtime context 与 JSON 编码后的选中文本组成；手动追问保持为普通用户消息；`Explain this` 消息作为带 `explain_selection` 意图的用户消息保存，并在请求时套用独立 runtime context。保存 Markdown 文件后，下一次请求会直接使用新内容。删除运行时模板并重启 Gloss，会恢复当前打包版本的默认文件。

若模板改动需要体现在新会话元数据和缓存键中，请同步递增 `src-tauri/src/sessions.rs` 内的 `PROMPT_VERSION`。

## 本地数据

默认数据目录：

```text
%APPDATA%\com.gloss.desktop\
├── settings.json
├── oauth.json
├── learner-profile.json
├── prompts\
└── sessions\
    └── <session-id>.jsonl
```

- `oauth.json` 明文保存 OAuth 令牌；会话 JSONL 保存对话和请求元数据。
- 每次会话更新都会写入对应的 JSONL；历史页面中的删除操作会移除该文件。
- `learner-profile.json` 基于 Triage 后的多轮对话更新。初始选中文本用于提供上下文，英语水平证据取自用户在后续对话中的表达。

## 代理行为

设置支持 HTTP、HTTPS、SOCKS5 和 SOCKS5H。`host:port` 形式会被规范化为 HTTP URL。代理用于：

- OAuth 令牌交换与刷新
- Responses 请求与继续提问
- 后台 CEFR 画像更新

浏览器授权页使用浏览器自身的网络设置；Gloss 的代理设置负责后续令牌交换和 AI 请求。

## OAuth 来源与许可

Rust OAuth 实现直接移植了 `oauth-cli-kit` 暴露的登录流程。依赖归属和许可信息见 [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md)。
