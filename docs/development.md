# Gloss 开发文档

本文面向 Gloss 的开发者和维护者。用户安装、使用和故障排查说明请阅读仓库根目录的 [README](../README.md)。

## 产品边界

Gloss 是轻量阅读与写作 Agent。它读取用户主动选中的文本，通过 ChatGPT Codex Responses 接口执行 Triage、自适应中英互译、英文修正和围绕原文的多轮对话。Windows 提供安装包，macOS 支持从源码构建；两端沿用相同的界面与业务流程。

## 技术栈

- 原生壳层：Rust、Tauri 2
- 前端：React 19、TypeScript、Vite
- 文本选区：Windows UI Automation `TextPattern`；macOS 辅助功能接口与必要的复制回退
- AI 接口：ChatGPT OAuth、Codex Responses endpoint
- 默认模型：`gpt-5.6-luna`
- 会话存储：每个会话一个 JSONL 文件
- 安装包：Windows NSIS x64；macOS `.app` / `.dmg`

## 代码结构

| 路径 | 职责 |
| --- | --- |
| `src/App.tsx` | 浮动工具栏、结果卡片、历史和设置界面 |
| `src/App.css` | 黑白主题、组件和窗口表面样式 |
| `src/markdown.ts` | Markdown 渲染兼容处理 |
| `src-tauri/src/lib.rs` | Tauri 生命周期、命令注册和托盘入口 |
| `src-tauri/src/selection.rs` | 选区数据结构及平台读取入口 |
| `src-tauri/src/overlay.rs` | 工具栏与卡片窗口的尺寸和位置 |
| `src-tauri/src/oauth.rs` | ChatGPT OAuth 登录、刷新和本地凭据 |
| `src-tauri/src/network.rs` | HTTP 与 SOCKS 代理配置 |
| `src-tauri/src/responses.rs` | Responses 流式请求、解析与重试 |
| `src-tauri/src/sessions.rs` | 会话模型、请求体和 JSONL 持久化 |
| `src-tauri/src/agent.rs` | Triage、Translate、Correct、继续提问和事件流 |
| `src-tauri/src/profile.rs` | CEFR 学习画像更新与合并 |
| `src-tauri/src/prompts.rs` | System Prompt 与运行时上下文模板加载 |
| `src-tauri/prompts/` | 随应用打包的 Triage、Translate、Correct 等默认 Prompt 模板 |

## 开发环境

需要准备：

- Windows 10/11，或用于验证移植的 macOS 开发机
- Node.js
- pnpm
- Rust 工具链及目标平台上的 Tauri 2 构建依赖；macOS 需要 Xcode Command Line Tools

安装依赖并启动开发版本：

```powershell
pnpm install --frozen-lockfile
pnpm tauri dev
```

使用内置示例数据启动浏览器预览：

```powershell
pnpm dev
```

macOS 构建应在 Mac 上执行。跨主机挂载源码目录不会改变命令的执行平台。Tauri 会自动将 `tauri.macos.conf.json` 合并到通用配置并覆盖安装包目标；其中 `resources: null` 清除仅供 Windows 使用的 WebView2 DLL 资源映射。macOS 原生依赖由 Cargo 的目标平台配置启用。透明窗口所需的 `macos-private-api` 特性与 `app.macOSPrivateApi` 在通用清单/配置中保持一致，满足 Tauri 的构建校验；相关原生行为仅用于 macOS。

## 验证

提交代码前至少运行：

```powershell
pnpm build
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

涉及全局快捷键、选区读取、透明窗口、拖动区域、托盘或 OAuth 回调的修改，还需要在对应平台的真实桌面环境中验证。修改共享逻辑时也要回归 Windows；SSH 中完成构建和测试不等于桌面行为已验证。

macOS 首轮验收使用 Chrome 网页和 Zotero。Chrome 正常使用是最低可用条件，Zotero 的 PDF 取词另记录实际结果。重点检查：

- 普通网页、输入框及可选中文字的 PDF；空选区、无辅助功能权限和应用切换时给出正确结果。
- 复制回退不会读取旧剪贴板，捕获后保留原有剪贴板格式；超时后不会继续向其他应用发送复制操作。
- 浮窗在 Retina、多屏及全屏应用所在桌面的位置与焦点；工具栏展开、收起和隐藏。
- 原有三种模式、Triage 追问、Explain this、Got it、学习画像、回答复制和错误重试。
- 真实登录与回调、历史/设置的重启恢复、可编辑 Prompt、代理、主题、自定义快捷键及开机启动。

最终还需验证打包后的 `.app`。开发运行、单元测试和已安装应用具有不同的运行方式，不能互相替代。实际支持范围以实机验证结果为准。

取词等待预算与原生调用取消是两回事：外层超时停止等待，不能取消正在运行的同步剪贴板 IPC。macOS 取词需在原生调用之间检查期限，并在发送复制前复核目标应用。剪贴板恢复通过变化计数检测并发修改；AppKit 没有提供写入者身份或比较后原子恢复操作，因此后台剪贴板管理器参与时仍需实测，不能宣称所有并发场景都能无损恢复。

### macOS 验证记录

2026-09-17，Apple Silicon / macOS 26.4，Chrome 152.0.7977.83、Zotero 9.0.6：本地测试者在已安装的 release 应用包中完成以下八项手工验收，均未发现问题。

- ChatGPT 登录及连接状态。
- Chrome 网页选区与 Translate 完整响应。
- Triage 首轮响应及继续追问。
- Correct 纠错。
- 复制回答并粘贴核对。
- Esc 隐藏后不点击其他应用，再次快捷键取词。
- 退出并重新启动后的登录、历史与继续会话。
- Zotero 可选中文字 PDF 的取词与翻译。

同一版代码通过前端构建、30 项 Rust 测试、严格 clippy、release `.app` 打包与签名校验。上述桌面结果来自手工验收；桌面自动化未完成全流程。Windows 的本轮构建与桌面回归、Intel Mac、其他系统版本、混合 DPI / 全屏、多格式剪贴板并发恢复，以及 Explain this / Got it、学习画像、主题、自定义快捷键、代理和开机启动等扩展回归尚未形成完整验证记录。

## 构建发布版本

```powershell
pnpm tauri build
```

Windows 主要产物：

```text
src-tauri\target\release\gloss.exe
src-tauri\target\release\bundle\nsis\Gloss_<version>_x64-setup.exe
```

构建前若 release 目录中的 `gloss.exe` 正在运行，需要先关闭该进程，否则 Windows 会阻止覆盖文件。

在 Mac 上只构建应用包可运行：

```sh
pnpm tauri build --bundles app
```

应用位于 `src-tauri/target/release/bundle/macos/Gloss.app`；默认 `pnpm tauri build` 还会构建 DMG。复制到固定位置后启动应用，再到系统设置的“隐私与安全性 → 辅助功能”允许 Gloss 读取选中文字。开机启动的验证应使用固定位置的应用包。

Mac 配置默认使用 ad-hoc 签名，供本地构建和自用；构建后可用 `codesign --verify --deep --strict` 检查应用包。正式分发时按 [Tauri 签名说明](https://v2.tauri.app/distribute/sign/macos/) 配置发布者的签名身份与公证。重新构建后仍应验证已授予的辅助功能权限是否有效。

### macOS 更新后已授权却仍无法取词

当前 ad-hoc 签名的身份要求绑定应用的代码哈希。更新后，辅助功能开关可能仍为开，但允许记录保存的是旧包签名；切换开关和重启未必更新这个绑定。2026-09-19 已实际复现并验证以下恢复步骤：

1. 先查看 `~/Library/Application Support/com.gloss.desktop/gloss.log`。若错误为 `Gloss needs Accessibility permission ...`，失败发生在读取选区之前，先处理授权；若已出现 `selection capture completed`，则沿后续动作或请求链路排查。
2. 在“系统设置 → 隐私与安全性 → 辅助功能”选中 Gloss，用减号移除旧条目。
3. 用加号添加固定安装位置中的当前 `Gloss.app`，开启权限，按系统要求完成验证。
4. 回到原应用选中文字并按全局快捷键，确认工具栏读到文本，且日志出现 `selection capture completed`。开关为开、重启成功和构建成功都不能单独代替取词验收。

本次移除并重新添加后，授权记录的代码要求与当前包一致，实际快捷键捕获 87 个字符，用户确认修复。无需修改选区读取或复制回退逻辑。界面须保留具体失败原因及完整说明入口，不能把权限、超时等所有错误都显示成 `No readable selection`。

本地安装完成后，注销并清理可重新生成的构建目录 `.app`；需要保留回退版本时使用 ZIP 等压缩档，避免多个同名应用被系统注册。公开安装包、源码与用户数据无需因此删除。

## 运行流程

1. 全局快捷键触发选区捕获。
2. 选区读取由目标平台实现。Windows 依次检查焦点元素和鼠标元素的祖先；UIA 无法暴露选区时，等待快捷键修饰键释放，再使用 `Ctrl+Insert` 读取并恢复剪贴板。macOS 优先读取辅助功能选区，必要时使用 `Command+C` 回退。两端向后续流程提供相同的文字与可选位置结构。
3. `overlay.rs` 在选区附近显示工具栏；无法取得可靠锚点时使用回退位置。
4. 用户选择 Triage、Translate 或 Correct 后，`agent.rs` 创建会话并组装 Responses 请求。
5. `responses.rs` 通过事件流把增量结果发送给 React 界面。
6. `sessions.rs` 将完整会话写入 JSONL。
7. Triage 发生继续提问、`Explain this` 或 `Got it` 学习信号后，`profile.rs` 会在后台评估并更新 CEFR 画像。

## Responses 接口与重试

请求体使用 Responses API 的消息与输出项结构，`store` 固定为 `false`。继续提问时会重放当前会话需要的完整消息，每次请求都能根据本地会话独立完成。

网络传输、暂时性 HTTP 状态或不完整流式响应最多自动重试两次。收到未授权响应时会尝试刷新 OAuth 令牌；仍然失败后，界面会保留可用的部分输出并提供手动重试。

模型和 Prompt 元数据定义在 `src-tauri/src/sessions.rs`：

```rust
pub const MODEL: &str = "gpt-5.6-luna";
pub const PROMPT_VERSION: u32 = 6;
```

## Prompt 模板

打包默认值位于 `src-tauri/prompts/`：

- `system.md`：定义 Gloss 的身份、对话行为和学习画像使用方式
- `triage.md`：首轮 Triage runtime context
- `translate.md`：首轮自适应中英互译 runtime context
- `correct.md`：首轮英文语法与自然度修正 runtime context
- `explain-selection.md`：Triage 结果划词后的快捷解释 runtime context
- `got-it.md`：Triage 结果划词后的已理解学习信号 runtime context
- `learner-profile.md`：根据多轮 Triage 对话生成 CEFR 画像补丁

首次启动时，Gloss 会把缺失的模板复制到：

```text
%APPDATA%\com.gloss.desktop\prompts\
```

macOS 对应位置为 `~/Library/Application Support/com.gloss.desktop/prompts/`。

每次请求都会重新读取模板。首条用户消息由 action runtime context 与 JSON 编码后的选中文本组成；Translate 会按主要语言自动选择中文→英文或英文→简体中文，不支持第三种目标语言；手动追问保持为普通用户消息；`Explain this` 和 `Got it` 分别作为带 `explain_selection`、`got_it` 意图的用户消息保存，并在请求时套用各自的 runtime context。保存 Markdown 文件后，下一次请求会直接使用新内容。删除运行时模板并重启 Gloss，会恢复当前打包版本的默认文件。应用升级时，未修改的旧版 Translate 模板会自动迁移，用户自定义模板则保持不变。

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

macOS 数据根目录为 `~/Library/Application Support/com.gloss.desktop/`，内部文件结构相同。应用负责自己的数据文件；开发依赖与构建缓存位于项目的 `node_modules/`、`dist/` 和 `src-tauri/target/`，可重新生成，不应打包进用户数据目录。

- `oauth.json` 明文保存 OAuth 令牌；会话 JSONL 保存对话和请求元数据。
- 每次会话更新都会写入对应的 JSONL；历史页面中的删除操作会移除该文件。
- `learner-profile.json` 基于 Triage 后的追问、划词解释和已理解信号更新。初始选中文本用于提供上下文，英语水平证据取自用户在后续对话中的表达和显式学习反馈。

## 代理行为

设置支持 HTTP、HTTPS、SOCKS5 和 SOCKS5H。`host:port` 形式会被规范化为 HTTP URL。代理用于：

- OAuth 令牌交换与刷新
- Responses 请求与继续提问
- 后台 CEFR 画像更新

浏览器授权页使用浏览器自身的网络设置；Gloss 的代理设置负责后续令牌交换和 AI 请求。

## OAuth 来源与许可

Rust OAuth 实现直接移植了 `oauth-cli-kit` 暴露的登录流程。依赖归属和许可信息见 [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md)。
