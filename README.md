# Gloss

Gloss 是一款 Windows 英文阅读助手。选中任意应用里的英文文本，按下全局快捷键，即可快速理解原文、翻译内容，并围绕原文继续提问。

它通过 Windows UI Automation 读取选区。

## 功能

- **Triage**：快速解释原文含义，并按你的英语水平补充词汇、语法、语用和文化背景。
- **Translate**：将选中的英文翻译成自然中文。
- **继续提问**：完成 Triage 后，可以围绕当前原文多轮提问。
- **划词解释**：在 Triage 内容中选中片段，点击 **Explain this** 即可继续追问。
- **长期适应**：只有在 Triage 后发生了继续提问，Gloss 才会谨慎更新本地 CEFR（A1–C2）学习画像，用于调整之后的讲解深度。
- **本地历史**：自动保存完整会话，随时返回之前的阅读记录。
- **代理支持**：支持 HTTP、HTTPS、SOCKS5 和 SOCKS5H 代理。

## 系统要求

- Windows 10 或 Windows 11
- 能使用 Codex 的 ChatGPT 账号
- 目标应用需要通过 Windows UI Automation 暴露选中的文本

Gloss 使用 ChatGPT OAuth 登录。

## 安装

从项目的 Releases 页面下载最新 Windows x64 安装程序并运行。安装完成后启动 Gloss，它会驻留在 Windows 通知区域。

如果你希望从源码构建，请阅读[开发文档](docs/development.md)。

## 开始使用

1. 启动 Gloss。
2. 在浏览器、文档或其他应用中选中一段英文。
3. 按下 `Ctrl+Alt+Shift+T`。
4. 在浮动工具栏中选择 `Triage` 或 `Translate`。
5. 第一次使用 AI 功能时，按提示在浏览器中登录 ChatGPT。

Triage 完成后，窗口底部会出现输入框。你可以继续询问句子含义、语法、词汇或背景知识。

关闭浮动窗口只会隐藏 Gloss。若要完全退出，请使用通知区域中的 Gloss 菜单。

## 设置

点击浮动工具栏上的设置按钮，可以修改：

- **Global shortcut**：唤出 Gloss 的全局快捷键。
- **Theme**：跟随系统，或固定使用浅色、深色主题。
- **Proxy**：OAuth 令牌交换和 AI 请求使用的代理。
- **ChatGPT**：登录或退出 ChatGPT。

代理地址可以直接填写 `127.0.0.1:23458`，Gloss 会将其识别为 `http://127.0.0.1:23458`。浏览器中的 ChatGPT 授权页面仍使用浏览器自己的网络设置。

## 隐私与本地数据

- Gloss 将当前选中的文本和当前会话消息发送给 ChatGPT。
- 历史记录、设置、学习画像和可编辑 Prompt 均保存在本机。
- OAuth 凭据保存在 Gloss 的本地应用数据目录中，目前为明文 JSON 文件。请保护好你的 Windows 账户和应用数据目录。
- 历史页面中的删除操作会移除对应的本地会话文件。

## 常见问题

### 提示 “No readable selection”

当前应用可能没有通过 Windows UI Automation 暴露文本选区。请确认文本仍处于选中状态，或尝试在其他浏览器、编辑器中使用。

### 按快捷键没有反应

快捷键可能被其他软件占用。打开 Gloss 设置，为 **Global shortcut** 录制一个新的组合键并保存。

### 无法登录或请求 ChatGPT

如果当前网络需要代理，请先在 **Settings → Network → Proxy** 中填写代理并保存，然后重试。浏览器授权页面需要单独使用浏览器自己的代理设置。

### 关闭窗口后 Gloss 去哪里了

Gloss 会继续在 Windows 通知区域运行。你可以再次按全局快捷键唤出它，或通过通知区域菜单打开设置和退出程序。

## 开发与许可信息

- [开发文档](docs/development.md)
- [第三方组件与许可说明](THIRD_PARTY_NOTICES.md)
