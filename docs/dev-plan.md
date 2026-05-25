# Flash Idea 开发计划

> 本文档是 AI 可执行的开发 spec。每个任务自包含，可独立并行执行。
> 执行方式：`codex exec -m gpt-5.5 "读 docs/dev-plan.md 的任务 N，执行"` 或 CC 直接执行。

## 前置条件

- Rust 1.95+, Tauri CLI 2.11+, Android NDK 27, Node.js 22+
- 环境变量已配置：ANDROID_HOME, NDK_HOME, JAVA_HOME
- 飞书开放平台自建应用已创建，有 app_id 和 app_secret

## 接口契约（所有任务共享）

### feishu-client crate 公开接口

```rust
// crates/feishu-client/src/lib.rs

pub struct FeishuClient {
    app_id: String,
    app_secret: String,
    // 内部管理 token 缓存和刷新
}

impl FeishuClient {
    /// 创建客户端实例
    pub fn new(app_id: String, app_secret: String) -> Self;

    /// 向指定文档追加一个文本段落
    /// client_token 用于幂等（传入消息 UUID）
    /// 返回 Ok(()) 或具体错误类型
    pub async fn append_text(
        &self,
        document_id: &str,
        content: &str,
        client_token: &str,
    ) -> Result<(), FeishuError>;
}

pub enum FeishuError {
    /// token 获取/刷新失败
    AuthError(String),
    /// 限频（HTTP 429 或 99991400），调用方应重试
    RateLimited,
    /// 网络错误，调用方应重试
    NetworkError(String),
    /// API 返回的业务错误（文档不存在、权限不足等），不应重试
    ApiError { code: i64, msg: String },
}
```

### Tauri Command 签名

```rust
// src-tauri/src/main.rs 或 src-tauri/src/commands.rs

#[tauri::command]
async fn send_message(text: String, state: State<'_, AppState>) -> Result<MessageResponse, String>;

#[tauri::command]
async fn get_messages(limit: Option<i64>, state: State<'_, AppState>) -> Result<Vec<Message>, String>;

#[tauri::command]
async fn retry_message(id: String, state: State<'_, AppState>) -> Result<(), String>;
```

```typescript
// 前端调用方式
interface MessageResponse {
  id: string;
  status: "queued" | "rejected";
}

interface Message {
  id: string;
  text: string;
  created_at: string;      // ISO 8601
  sync_status: string;     // queued / synced / failed
}
```

### Tauri Event

```
事件名: "sync_status_changed"
载荷: { id: string, status: "synced" | "failed" }
```

### SQLite Schema

```sql
CREATE TABLE IF NOT EXISTS messages (
  id TEXT PRIMARY KEY,
  text TEXT NOT NULL,
  created_at TEXT NOT NULL,
  sync_status TEXT NOT NULL DEFAULT 'queued',
  retry_count INTEGER NOT NULL DEFAULT 0,
  target_doc_id TEXT,
  metadata TEXT NOT NULL DEFAULT '{}',
  synced_at TEXT
);
```

---

## 任务 1：Cargo Workspace 骨架 + 项目配置

**输出文件：**
- `Cargo.toml`（workspace root）
- `crates/feishu-client/Cargo.toml`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`
- `src-tauri/capabilities/default.json`
- `.env.example`
- `.gitignore`
- `src/index.html`（最小占位，任务 3 会覆盖）

**具体要求：**

1. 根 `Cargo.toml` 定义 workspace，members 为 `["crates/feishu-client", "src-tauri"]`

2. `crates/feishu-client/Cargo.toml`:
   - name = "feishu-client"
   - 依赖：reqwest（features: json, rustls-tls）, serde + serde_json, tokio（features: sync）, thiserror
   - 不依赖 tauri

3. `src-tauri/Cargo.toml`:
   - 依赖 tauri（features: 按 tauri 2.x 默认）、feishu-client（path）、rusqlite（features: bundled）、uuid（features: v4）、serde + serde_json、tokio、chrono、dotenvy
   - tauri plugin 按需加（tauri-plugin-shell 等）

4. `src-tauri/tauri.conf.json`:
   - productName: "Flash Idea"
   - identifier: "com.flashidea.app"
   - build.devUrl: "http://localhost:1420"
   - build.frontendDist: "../src"
   - app.windows: 单窗口，title "Flash Idea"，宽 400 高 700

5. `.env.example`:
   ```
   FEISHU_APP_ID=cli_xxxxxxxxxx
   FEISHU_APP_SECRET=xxxxxxxxxxxxxxxxxxxxxxxx
   FEISHU_DOC_ID=dcnxxxxxxxxxxxxxxxx
   ```

6. `.gitignore`: target/, node_modules/, .env, *.apk, dist/

**验收：** `cargo check --workspace` 通过（feishu-client 和 src-tauri 都能编译，src-tauri 里 main.rs 可以是空的 `fn main() {}`占位）。

---

## 任务 2：feishu-client crate

**输出文件：**
- `crates/feishu-client/src/lib.rs`

**具体要求：**

实现上面接口契约中的 `FeishuClient` 和 `FeishuError`。

1. **Token 管理**：
   - 内部用 `tokio::sync::RwLock` 缓存 token 和过期时间
   - `get_token()` 内部方法：如果 token 有效且剩余 > 30 分钟，返回缓存；否则调用 `POST /open-apis/auth/v3/tenant_access_token/internal` 刷新
   - 刷新失败返回 `FeishuError::AuthError`

2. **append_text**：
   - 调用 `POST /open-apis/docx/v1/documents/{document_id}/blocks/{document_id}/children`
   - query 参数加 `client_token`（传入的消息 UUID，实现幂等）
   - body: `{ "children": [{ "block_type": 2, "text": { "elements": [{ "text_run": { "content": "<content>", "text_element_style": {} } }], "style": {} } }] }`
   - HTTP 429 或 error code 99991400 → `FeishuError::RateLimited`
   - HTTP 401 → 清除 token 缓存，返回 `FeishuError::AuthError`
   - 网络超时/连接失败 → `FeishuError::NetworkError`
   - 其他 API 错误 → `FeishuError::ApiError`

3. **HTTP 客户端**：用 reqwest::Client，构造时设 timeout 10 秒，复用同一个 Client 实例。

4. **不要**依赖 tauri、不要读文件系统、不要读环境变量。app_id 和 app_secret 通过构造函数传入。

**验收：** `cargo test -p feishu-client` 通过。至少写 2 个单元测试：
- 测试 token 缓存逻辑（mock HTTP 或用 test helper）
- 测试 append_text 构造的请求体格式正确

---

## 任务 3：前端 UI（聊天气泡界面）

**输出文件：**
- `src/index.html`
- `src/style.css`
- `src/app.js`

**具体要求：**

纯 HTML + CSS + JS，不用框架。界面模仿微信/飞书单聊。

1. **布局**：
   - 顶部标题栏："Flash Idea"，固定在顶部
   - 中间消息列表：可滚动，消息靠右（自己发的）
   - 底部输入区：输入框 + 发送按钮，固定在底部

2. **消息气泡**：
   - 每条消息显示：文本内容 + 时间（HH:MM）+ 状态图标
   - 状态图标：⏳（queued）、✓（synced）、✗（failed，可点击重试）
   - 新消息自动滚动到底部

3. **交互**：
   - 发送按钮点击或 Enter 键触发发送
   - 空文本不发送
   - 发送后立即清空输入框
   - 调用 Tauri IPC：`window.__TAURI__.core.invoke('send_message', { text })`
   - 返回后在列表中添加气泡（status = queued）
   - 监听 Event：`window.__TAURI__.event.listen('sync_status_changed', callback)`
   - 收到事件后更新对应气泡的状态图标
   - 点击 ✗ 图标调用 `invoke('retry_message', { id })`

4. **启动时**：
   - 调用 `invoke('get_messages', { limit: 50 })` 加载历史
   - 渲染到消息列表

5. **样式要求**：
   - 移动端适配，viewport meta 设好
   - 气泡圆角、浅色背景，整体简洁
   - 输入框高度适中，方便触屏点击
   - 深色模式 prefers-color-scheme 支持（可选，不强求）

**验收：** 浏览器打开 `src/index.html` 能看到完整界面（Tauri IPC 调用会失败但 UI 正常渲染）。

---

## 任务 4：Tauri App 层（Command + SQLite + 同步）

**输出文件：**
- `src-tauri/src/main.rs`
- `src-tauri/src/commands.rs`
- `src-tauri/src/db.rs`
- `src-tauri/src/sync.rs`

**具体要求：**

1. **main.rs**：
   - 读取 `.env`（用 dotenvy）
   - 初始化 SQLite（调用 db 模块）
   - 构造 FeishuClient（从环境变量读 app_id, app_secret）
   - 构造 AppState（包含 db 连接、FeishuClient、doc_id）
   - tauri::Builder 注册 commands、管理 state、运行

2. **db.rs**：
   - `init_db(path) -> Connection`：创建表（用上面的 schema）
   - `insert_message(conn, id, text, created_at, target_doc_id) -> Result`
   - `get_messages(conn, limit) -> Vec<Message>`：按 created_at 降序取，返回时反转为升序
   - `get_queued_messages(conn) -> Vec<Message>`：取所有 sync_status = 'queued' 的
   - `update_sync_status(conn, id, status, synced_at) -> Result`
   - `increment_retry(conn, id) -> Result`
   - 用 `rusqlite`，Connection 用 `Mutex<Connection>` 包装

3. **commands.rs**：
   - `send_message`：校验非空 → 生成 UUID → 写 SQLite（queued）→ 返回 MessageResponse → 异步触发同步（spawn 一个 tokio task，不阻塞 command 返回）
   - `get_messages`：从 SQLite 读取
   - `retry_message`：重置 sync_status 为 queued、retry_count 为 0 → 触发同步

4. **sync.rs**：
   - `sync_message(feishu_client, db, doc_id, message, app_handle)`：
     - 调用 `feishu_client.append_text(doc_id, formatted_content, message.id)`
     - formatted_content 格式：`[HH:MM:SS] {text}`（从 message.created_at 提取时间）
     - 成功 → 更新 SQLite 为 synced + synced_at → emit "sync_status_changed" { id, status: "synced" }
     - RateLimited → 等 350ms 后重试（最多 1 次额外重试）
     - NetworkError → increment retry_count，如果 < 5 保持 queued，>= 5 标记 failed → emit
     - AuthError / ApiError → 标记 failed → emit
   - `sync_all_queued(feishu_client, db, doc_id, app_handle)`：
     - 取所有 queued 消息，逐条调用 sync_message，每条间隔 350ms
     - 在 App 恢复前台时调用

**验收：** `cargo build -p flash-idea`（src-tauri 的 package name）编译通过。

---

## 执行顺序

```
任务 1（骨架）─── 必须先完成
    ├── 任务 2（feishu-client）──┐
    ├── 任务 3（前端 UI）────────┼── 并行执行
    └── 任务 4（Tauri app 层）───┘
```

任务 1 完成后，2/3/4 可以并行。任务 4 依赖任务 2 的接口签名（已在本文档定义），但不需要任务 2 编译通过——只要接口契约一致，各自编译时引用 path 依赖即可。

## 合并后验收

所有任务完成合并后：
1. `cargo check --workspace` 通过
2. `cargo test --workspace` 通过
3. `cargo tauri dev` 能启动桌面窗口，显示聊天界面
4. 发送消息 → SQLite 写入成功 → 气泡出现
5. 配置真实飞书凭证后 → 消息出现在飞书文档末尾
