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

## 技术栈

- **Tauri 2.0**（Rust 后端 + Web 前端）
- Rust: reqwest, rusqlite, tokio, serde
- 前端: 纯 HTML/CSS/JS，无框架
- 飞书开放平台 API（docx v1）

## 快速开始

### 前置条件

- Rust 1.95+
- Tauri CLI 2.11+（`cargo install tauri-cli`）
- 飞书开放平台[自建应用](https://open.feishu.cn/app)，获取 App ID 和 App Secret
- 在飞书知识库创建一个父节点，记下 URL 中的 node_token

### 桌面端

```bash
# 配置环境变量
cp .env.example .env
# 编辑 .env 填入飞书凭据

# 开发模式
cargo tauri dev

# 构建
cargo tauri build
```

### Android

环境要求：Java 17、Android SDK、NDK、Rust aarch64-linux-android target

```bash
export JAVA_HOME="/opt/homebrew/Cellar/openjdk@17/17.0.15/libexec/openjdk.jdk/Contents/Home"
export ANDROID_HOME="$HOME/Library/Android/sdk"
export NDK_HOME="$ANDROID_HOME/ndk/$(ls $ANDROID_HOME/ndk/ | head -1)"

# 构建 debug APK
cargo tauri android build --apk --debug --target aarch64

# 安装（覆盖安装保留数据）
adb install -r src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
```

Android 端的飞书凭据在应用内设置页面配置（无需 .env 文件）。

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

## License

MIT
