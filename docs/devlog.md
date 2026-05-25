# FlashIdea 开发日志

## v0.2.1 — 2026-05-25

### 修复

- **Android 白屏**：启动窗口背景色匹配应用主题（浅米 / 暗色），HTML 加载状态消除闪屏
- **消息排序**：云端同步消息按 `created_at` 正确排列，不再追加到列表末尾
- **Android 键盘交互**：`visualViewport` 动态高度 + `adjustResize`，微信风格输入体验
- **版本号流程**：每次改动 bump patch，`adb install -r` 覆盖安装保留数据

### GitHub Release

- Mac: `FlashIdea_0.2.1_aarch64.dmg`（4.9MB）
- Android: `FlashIdea_0.2.1_arm64.apk`（157MB，debug 签名）
- https://github.com/MuChengZJU/FlashIdea/releases/tag/v0.2.1

---

## v0.2.0 — 2026-05-19

### 本版本新增

- **多端同步**：创建文档前查飞书子节点避免重复；启动时拉取云端文档内容补全本地历史
- **新增飞书 API**：`list_wiki_children`（分页）、`get_document_raw_content`
- **远程消息去重**：按 text + doc_id 匹配，metadata 标记 `source: remote`

---

## v0.1.0 — 2026-05-19

### 已完成

- **飞书同步链路打通**（Mac）：Token → Wiki 节点查询 → 创建子文档 → 写入文本，全链路通过
  - 根因修复：`create_wiki_child` 缺少 `node_type: "origin"` 字段
  - 写了 `cargo run --example diagnose` 诊断工具定位问题
- **Android APK 构建成功**：拆 `main.rs` → `lib.rs`（Tauri Android 需要 cdylib），配阿里云 Maven 镜像
- **配置界面**：应用内飞书凭据配置，存 SQLite settings 表，支持测试连接
  - 优先级：环境变量 > SQLite，Secret 脱敏
- **Android 启动崩溃修复**：DB 初始化移到 `setup()` 内，Android 用 `app.path().app_data_dir()`

### 待解决

#### ~~P0: 多端同步 — 每个设备重复创建当日文档~~ ✅ 已修复

**现象**：Mac 创建了 "FlashIdea - 2026-05-19"，手机端因为本地 SQLite 没有 `active_doc_id`，又创建了一个同名文档。

**根因**：`resolve_doc_id` 只查本地 `active_doc_id` 和 `last_synced_at`，新设备没有本地状态就直接 `create_wiki_child`。

**修复**：
1. 防重复创建：`resolve_doc_id` 创建子文档前，先调用 `list_wiki_children` 列出父节点下所有子节点，按标题匹配当日文档。找到就复用其 `obj_token`，找不到才创建
2. 云端拉取：启动时调用 `pull_remote_messages`，读取当日文档的原始文本，解析 `[HH:MM:SS] text` 格式，按 text + doc_id 去重后插入本地 SQLite，前端收到 `messages_updated` 事件自动刷新

新增 API：`list_wiki_children`（支持分页）、`get_document_raw_content`

#### P1: 手机端 WebView 键盘交互

**现象**：点击输入框后键盘弹出，整个页面上移/错位，输入区域可能被遮挡或布局异常。

**根因**：Android WebView 处理虚拟键盘时的 viewport 行为和桌面浏览器不同，`100dvh` / `100vh` 在键盘弹出时的表现不一致。

**方案方向**：
- 使用 `visualViewport` API 监听键盘高度变化，动态调整布局
- AndroidManifest 的 `windowSoftInputMode` 设置（`adjustResize` vs `adjustPan`）
- CSS `env(keyboard-inset-bottom)` 或 JS polyfill
- 需要在真机上实际调试交互细节

### Android 构建 & 安装

环境要求：Java 17、Android SDK、NDK、Rust aarch64-linux-android target

```bash
# 设环境变量
export PATH="$HOME/.cargo/bin:$HOME/Library/Android/sdk/platform-tools:$PATH"
export JAVA_HOME="/opt/homebrew/Cellar/openjdk@17/17.0.15/libexec/openjdk.jdk/Contents/Home"
export ANDROID_HOME="$HOME/Library/Android/sdk"
export NDK_HOME="$ANDROID_HOME/ndk/$(ls $ANDROID_HOME/ndk/ | head -1)"

# 构建 debug APK（手机用这个）
cargo tauri android build --apk --debug

# 卸载旧版（避免签名冲突）+ 安装
adb shell pm uninstall com.flashidea.app
adb install src-tauri/gen/android/app/build/outputs/apk/arm64/debug/app-arm64-debug.apk
```

踩坑记录：
- `/usr/libexec/java_home` 默认返回 Java 11（Corretto），Gradle 需要 Java 17，必须手动指定 `JAVA_HOME`
- release APK 未签名无法安装，开发阶段用 `--debug`
- 首次安装或签名变更时先 `adb shell pm uninstall`，否则报 Failure [-99]
- 目标手机 OPPO Find X7 Ultra 是 arm64，产物在 `apk/arm64/debug/` 下

### 关键设计决策

| 决策 | 选项 | 选择 | 原因 |
|------|------|------|------|
| 文档分割时间 | 自然日 / 6小时间隔 / 06:00 | 06:00 本地时间 | 凌晨创作属于"昨天"更符合直觉 |
| 凭据存储 | Keychain / SQLite / SharedPrefs | SQLite settings 表 | 已有基础设施，Android app 私有目录足够安全 |
| 配置优先级 | 环境变量 only / SQLite only | 环境变量 > SQLite | 桌面开发用 .env 方便，手机端用 SQLite |
