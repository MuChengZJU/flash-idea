# FlashIdea

零摩擦语音捕捉管道。点击图标 → 聊天气泡界面 → 语音转文字（系统输入法）→ 发送 → 自动追加到飞书云文档。

受锤子闪念胶囊启发，但不做 AI 助手，只做语音便签本。

## 功能

- 聊天气泡 UI，发送即存
- 每条消息以 `[HH:MM:SS] 内容` 格式写入飞书云文档
- 离线缓存，恢复网络后自动同步
- 多端同步：启动时拉取云端文档内容，自动补全本地历史
- 每日自动创建新文档（06:00 为分界），多设备不重复创建
- 应用内配置飞书凭据，支持测试连接

## 安装

从 [Releases](https://github.com/MuChengZJU/FlashIdea/releases) 下载最新版本：

| 平台 | 文件 | 说明 |
|------|------|------|
| macOS (Apple Silicon) | `FlashIdea_x.x.x_aarch64.dmg` | 双击打开，拖入 Applications |
| Android (arm64) | `FlashIdea_x.x.x_arm64.apk` | 下载后直接安装，需允许"未知来源" |

### macOS 安装

1. 下载 `.dmg` 文件
2. 双击打开，将 FlashIdea 拖入 Applications 文件夹
3. 首次打开如遇"无法验证开发者"提示：系统设置 → 隐私与安全性 → 点击"仍要打开"

### Android 安装

1. 下载 `.apk` 文件到手机
2. 点击安装，如提示"未知来源"需在设置中允许
3. 安装完成后打开即可使用

## 配置

首次打开会进入配置页面，需要填写飞书凭据：

### 1. 创建飞书自建应用

1. 打开 [飞书开放平台](https://open.feishu.cn/app)，创建企业自建应用
2. 在"权限管理"中添加权限：
   - `wiki:wiki` — 知识库读写
   - `docx:document` — 文档读写
3. 发布应用（审批通过后生效）
4. 在"凭证与基础信息"中获取 **App ID** 和 **App Secret**

### 2. 准备知识库节点

1. 在飞书知识库中创建一个文件夹/节点，作为 FlashIdea 的父目录
2. 打开该节点，从 URL 中提取 `node_token`（形如 `https://xxx.feishu.cn/wiki/XXXXX`，XXXXX 就是 token）
3. 确保你的自建应用有该知识库的访问权限（知识库设置 → 成员管理 → 添加应用）

### 3. 在 FlashIdea 中填入凭据

- 打开 FlashIdea → 点击右上角齿轮图标进入设置
- 填入 App ID、App Secret、知识库节点 Token
- 点击"测试连接"确认配置正确
- 点击"保存"

配置完成后即可使用。输入文字发送，消息会自动同步到飞书云文档。

## 使用建议

- **语音输入**：推荐搭配豆包输入法（语音转文字准确率高），点击输入框后切换到语音输入即可
- **多设备**：Mac 和手机都安装，消息通过飞书云文档自动同步
- **查看记录**：直接在飞书知识库中查看，每天一篇文档，格式为 `FlashIdea - 2026-05-25`

## 从源码构建

### 前置条件

- Rust 1.95+
- Tauri CLI 2.11+（`cargo install tauri-cli`）

### macOS

```bash
git clone https://github.com/MuChengZJU/FlashIdea.git
cd FlashIdea

# 配置环境变量（开发时用）
cp .env.example .env
# 编辑 .env 填入飞书凭据

# 开发模式（热重载）
cargo tauri dev

# 构建 .dmg 安装包
cargo tauri build
# 产物在 target/release/bundle/dmg/
```

### Android

环境要求：Java 17、Android SDK、NDK

```bash
# macOS 环境变量（根据实际路径调整）
export JAVA_HOME="<java17路径>"
export ANDROID_HOME="$HOME/Library/Android/sdk"
export NDK_HOME="$ANDROID_HOME/ndk/<版本号>"

# 添加 Rust Android target
rustup target add aarch64-linux-android

# 初始化 Tauri Android 项目（首次）
cargo tauri android init

# 构建 debug APK
cargo tauri android build --apk --debug --target aarch64

# 产物在 src-tauri/gen/android/app/build/outputs/apk/universal/debug/
# 安装到手机
adb install -r src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
```

Android 端不需要 `.env` 文件，飞书凭据在应用内设置页面配置。

## 技术栈

- **Tauri 2.0**（Rust 后端 + Web 前端）
- Rust: reqwest, rusqlite, tokio, serde
- 前端: 纯 HTML/CSS/JS，无框架
- 飞书开放平台 API（docx v1）

## 项目结构

```
flashidea/
├── crates/feishu-client/     # 独立飞书 API 客户端 crate
├── src-tauri/src/             # Tauri Rust 后端
│   ├── lib.rs                 # 入口、初始化
│   ├── commands.rs            # IPC 命令
│   ├── db.rs                  # SQLite 操作
│   └── sync.rs                # 飞书同步逻辑
├── src/                       # Web 前端
│   ├── index.html
│   ├── style.css
│   └── app.js
└── docs/                      # 设计文档、开发日志
```

## 交流

加入 [Flash Idea 用户群](https://applink.feishu.cn/client/chat/chatter/add_by_link?link_token=8afj98e4-e240-4201-8cb4-b1d09ce6ba82)，反馈问题、交流想法：

<img src="docs/assets/feishu-group-qr.jpg" width="300" alt="Flash Idea 用户群二维码" />

## License

MIT
