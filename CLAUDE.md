# FlashIdea

零摩擦语音捕捉管道。点击图标 → 聊天气泡界面 → 语音转文字（豆包输入法）→ 发送 → 自动追加到飞书云文档。

## 技术栈

- Tauri 2.0（Rust 后端 + Web 前端）
- Rust: reqwest, rusqlite, tokio, serde
- 前端: 纯 HTML/CSS/JS，不用框架
- 飞书开放平台 API（docx v1）

## 项目结构

```
flashidea/
├── Cargo.toml                # workspace root
├── crates/
│   └── feishu-client/        # 独立 crate，无 Tauri 依赖
├── src-tauri/                # Tauri app
│   └── src/
│       ├── main.rs
│       ├── commands.rs
│       ├── db.rs
│       └── sync.rs
├── src/                      # Web 前端
│   ├── index.html
│   ├── style.css
│   └── app.js
└── docs/                     # 文档
    ├── design.md             # 产品/架构设计文档
    ├── visual-design.md      # 视觉设计系统（闪电琥珀）
    ├── dev-plan.md           # 开发计划（AI 可执行）
    └── feishu-api.md         # 飞书 API 速查
```

## 开发规范

- commit message 中文描述，英文前缀：`feat:` / `fix:` / `docs:` / `refactor:`
- 密钥放 `.env`，不进代码。`.env.example` 只放占位符
- Rust 代码用 `cargo fmt` 和 `cargo clippy`
- 前端不用 npm/打包工具，纯静态文件

## 关键设计决策

架构决策详见 `docs/design.md`，视觉设计详见 `docs/visual-design.md`。核心要点：
- IPC: Tauri Command（send_message 写 SQLite 后立即返回，不等网络）
- 同步通知: Command + Event 混合（SQLite 是状态真相来源）
- 飞书幂等: 用消息 UUID 作为 client_token
- 冷启动目标: < 500ms（T4 - T0）
- 视觉方向: 闪电琥珀（暖黄色调），设计锚点"秒开"

## 开发计划

详见 `docs/dev-plan.md`。4 个任务，任务 1 完成后 2/3/4 可并行。
