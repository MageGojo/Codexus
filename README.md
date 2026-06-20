# Codexus

> 为 Codex 接入任意模型 —— 一个跨平台（macOS / Windows）的 Codex 代理与多模型接入桌面应用。

**当前状态**

- ✅ **macOS**：已实现，可编译、打包、安装运行（Flutter + Rust）。
- ✅ **Windows**：跨平台代码已就绪，daemon 可交叉编译为 `ferry-daemon.exe`；GUI 由 CI（`windows-latest`）或 `scripts/package-windows.ps1` 在 Windows 上构建。

---

## 一句话定位

**Codexus** 是用 **Flutter（前端）+ Rust（后端）** 打造的原生桌面软件。它在本地起一个轻量代理网关 + 管理服务，让 OpenAI Codex（CLI / 桌面端 / IDE 插件）能够：

1. **登录管理**：统一管理 Codex 的 ChatGPT OAuth 登录与 API Key，支持多账号、标签、备注。
2. **中转接入**：本地代理接管 Codex 请求，做 Responses ↔ Chat Completions 协议转换与路由，支持官方直连与第三方中转。
3. **国内模型接入**：把 DeepSeek、通义千问、Kimi、智谱 GLM 等国产模型一键接入 Codex。
4. **会话与用量**：本地持久化每次请求/响应与会话历史，统计 token 用量、配额（5h/7d）、成功率，提供可检索、可回看、可导出的仪表盘。

---

## 功能特性

- **账号与登录**
  - ChatGPT OAuth 2.0 + PKCE 登录（本地 `127.0.0.1:1455/callback` 回调）、API Key 登录
  - 多种添加方式：浏览器 OAuth、API Key、粘贴 `auth.json` JSON 自动识别、导入本机 `~/.codex/auth.json`
  - 多账号管理、标签 / 备注、plan 徽章、Token 自动刷新、配额（5h/7d）查看与自动刷新
- **代理网关（中转 / 直连）**
  - 本地回环代理接管 Codex 流量，`Responses API` ↔ 上游 `Chat Completions API` 双向协议转换
  - 自动识别路由：OAuth 账号写 `~/.codex/auth.json` 直连官方；供应商 / 中转账号经本地代理转换
  - 多账号池、轮询、故障转移、配额感知策略、模型别名映射
- **国内模型接入**
  - 内置供应商预设：DeepSeek、通义千问(Qwen)、Kimi、智谱 GLM 等（可编辑、可覆盖）
  - 自定义 OpenAI 兼容 Base URL + Key，自动拉取上游 `/v1/models`
  - 一键写入 `~/.codex/config.toml`，无需手动改配置
- **会话与仪表盘**
  - 读取 Codex 会话 rollout 文件统计真实 token 用量；本地 SQLite 记录请求/响应、模型、耗时、状态
  - 会话历史浏览、搜索、聊天记录详情、导出（JSON / Markdown）
  - 仪表盘：累计 / 今日 Token、请求成功率、账号存活率、Token 增长曲线、中转掺假检测、天气 / 古诗词点缀
- **体验**
  - 液态玻璃风格的现代桌面 UI，深浅色主题
  - macOS 菜单栏托盘（Windows 暂用原生窗口）
  - 本地优先、凭据默认落本地文件（0600），可选 macOS Keychain

---

## 命名说明

**Codexus** = **Codex** + **nexus（枢纽）**：为 Codex 充当连接任意模型的中枢。

> 对外品牌统一为 Codexus。内部技术标识按契约**保留历史代号**（改动会破坏 `~/.codex/config.toml` 契约或触发数据迁移）：Rust crate `ferry-*`、数据目录 `~/.codexferry`、bundle id `com.codexferry.app`、配置键 `[model_providers.codexferry]` 等，均不影响对外品牌。

---

## 技术栈

| 层 | 选型 |
| --- | --- |
| 前端 GUI | Flutter（macOS / Windows desktop） |
| 后端核心 | Rust（代理网关 + 协议转换 + 存储 + 凭据 + 本地管理 API） |
| 进程模型 | Rust 常驻 sidecar（`ferry-daemon`，本地 HTTP 管理接口 `127.0.0.1:15722`），GUI 启动时自动拉起 |
| 存储 | SQLite（会话 / 统计） + 本地文件凭据（默认）/ 系统 Keychain（可选） |
| 打包 | macOS `.app`（ad-hoc 或 Developer ID 签名）、Windows `.exe` + 同目录 daemon |

---

## 目录结构

```text
Codexus/
├── README.md
├── LICENSE                       # MIT
├── .github/workflows/ci.yml      # CI：macOS + Windows 构建与测试
├── scripts/
│   ├── package-macos.sh          # 本地 macOS 构建 + 打包 + 签名
│   └── package-windows.ps1       # 本地 Windows 构建 + 打包
├── app/                          # Flutter 前端工程（macos/ + windows/ + lib/）
│   ├── lib/
│   │   ├── app/ ui/ models/ ipc/ platform/
│   │   └── main.dart
│   ├── macos/  windows/          # 各平台 runner 工程
│   └── pubspec.yaml
└── core/                         # Rust 后端核心（cargo workspace）
    ├── crates/
    │   ├── ferry-proxy/          # 代理网关、协议转换、账号池、tokenizer
    │   ├── ferry-convert/        # Responses ↔ Chat Completions 转换
    │   ├── ferry-auth/           # OAuth / API Key / 多账号 / 凭据存储
    │   ├── ferry-config/         # ~/.codex/config.toml 接管 / 备份 / 回滚
    │   ├── ferry-store/          # SQLite 会话与统计存储
    │   ├── ferry-codexlog/       # 解析 Codex rollout 会话文件统计用量
    │   ├── ferry-ipc/            # 本地管理 API（GUI <-> 核心）
    │   └── ferry-daemon/         # 常驻 sidecar 入口
    └── Cargo.toml
```

> 设计 / 进度文档保存在本地 `docs/`（不随仓库上传）。

---

## 构建与运行

### 前置

- [Rust](https://rustup.rs/)（stable）
- [Flutter](https://docs.flutter.dev/get-started/install)（stable）
- macOS：Xcode + CocoaPods
- Windows：Visual Studio 2022（含 Desktop development with C++ 工作负载）

### 一键打包

- **macOS**：在仓库根目录运行 `./scripts/package-macos.sh` → 产出 `Codexus.app` 并打包 `Codexus-macos.zip`。
- **Windows**：在仓库根目录的 PowerShell 运行 `.\scripts\package-windows.ps1` → 在 `app/build/windows/x64/runner/Release/` 产出 `codexferry.exe` + `ferry-daemon.exe`，并打包 `Codexus-windows-x64.zip`。

### 开发运行

```bash
# 1) 构建后端 daemon（GUI 会自动定位并拉起）
cd core && cargo build -p ferry-daemon

# 2) 运行前端（按平台二选一）
cd ../app
flutter run -d macos      # macOS
flutter run -d windows    # Windows
```

GUI 启动时会自动探测并拉起 `ferry-daemon`：打包态在 GUI 同目录（Windows）或 `.app/Contents/Resources/`（macOS），开发态从 `core/target/{debug,release}/` 定位。

### 持续集成

`.github/workflows/ci.yml` 会在每次 push / PR 时：在 macOS 上跑 `cargo test` + `flutter analyze` + `flutter test`，并在 `macos-latest` 与 `windows-latest` 上分别构建、打包桌面产物并上传为 CI artifact。

---

## 凭据与隐私

- 默认把凭据明文落本地文件（`~/.codexferry/auth/*.json`，权限 0600，目录 0700），等同 Codex CLI 的 `auth.json`；可设 `FERRY_AUTH_BACKEND=keychain` 改用系统 Keychain。
- 所有凭据与会话仅保存在本地，不上传任何第三方服务器；本地管理 API 默认绑定回环地址并要求 Bearer Token。

---

## 合规 / 免责声明

- 本项目仅作为**本地代理与配置管理工具**，不存储或转发用户凭据到第三方服务器。
- `auth.json`、API Key 等敏感信息等同于密码，应严格本地保管。
- 使用第三方中转或代理访问官方账号可能存在账号风险，需用户知情并自行承担；官方供应商建议直连。

---

## License

[MIT](LICENSE) © 2026 Codexus
