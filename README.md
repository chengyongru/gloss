# Gloss

Gloss 是 Windows 上的英文阅读助手。选中文本，按下全局快捷键，即可完成 Triage、翻译，并围绕原文继续提问。

## 功能

- **Triage**：给出贴合语境的中文理解，并挑选值得解释的词汇、表达、句式或背景。
- **Translate**：将选中的英文翻译成自然中文。
- **追问与划词解释**：围绕当前原文继续提问，或在 Triage 内容中选中片段并使用 **Explain this**。
- **学习画像**：根据 Triage 后的对话谨慎更新本地 CEFR（A1–C2）画像，用于调整后续讲解深度。
- **本地历史**：以 JSONL 保存完整会话。
- **代理**：支持 HTTP、HTTPS、SOCKS5 和 SOCKS5H。

## 安装

Gloss 支持 Windows 10/11，需要能够使用 Codex 的 ChatGPT 账号。

从 [Releases](../../releases) 下载 Windows x64 安装程序。启动后，Gloss 会驻留在 Windows 通知区域。

源码构建见[开发文档](docs/development.md)。

## 使用

默认快捷键是 `Ctrl+Alt+Shift+T`。首次请求会通过浏览器完成 ChatGPT OAuth 登录。Triage 结果支持继续提问；关闭浮动窗口只会隐藏 Gloss，完全退出使用通知区域菜单。

## 设置

代理可以填写完整 URL，也可以直接填写 `127.0.0.1:23458`；后者会按 `http://127.0.0.1:23458` 处理。代理用于 OAuth 令牌交换和 AI 请求，浏览器授权页面仍使用浏览器自身的网络设置。

## 数据

- 选中的文本和当前会话消息会发送给 ChatGPT。
- 历史、设置、学习画像和可编辑 Prompt 保存在本机。
- OAuth 凭据以明文 JSON 保存在 Gloss 应用数据目录中。
- 删除历史记录会移除对应的本地 JSONL 文件。

## 问题排查

- **No readable selection**：目标应用没有通过 UI Automation 或复制语义提供可读取的选区。
- **快捷键无响应**：组合键可能已被其他软件占用，在 Settings 中更换即可。
- **登录或请求失败**：检查 Settings 中的代理；浏览器授权页需要单独配置浏览器代理。

## 开发

- [开发文档](docs/development.md)
- [第三方组件说明](THIRD_PARTY_NOTICES.md)
