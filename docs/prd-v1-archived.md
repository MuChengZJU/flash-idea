# Flash Idea 飞快说

> 闪念胶囊 for 飞书 —— 极轻量跨平台语音 inbox，一键语音转文字，自动写入飞书云文档。

## 1. 项目背景与灵感

受锤子闪念胶囊启发：零摩擦捕捉想法，说完即存。但闪念胶囊已停更，且市面没有好的替代品。现有方案的问题：

- **飞书/微信原生**：启动慢、层级深、开屏加载，无法一步到达输入界面
- **录音硬件（飞书录音豆/Plaud）**：同步慢，数据锁在厂商 App 里，提取原文要点好多层
- **手机语音助手（小布等）**：语义理解层引入不必要的延迟和失败

Flash Idea 的定位：**不做 AI 助手，只做语音便签本**。一个极简的聊天气泡界面，用户说话 → 语音转文字（由系统输入法完成，推荐豆包输入法）→ 自动 append 到飞书云文档。

## 2. 核心功能（MVP）

1. **聊天气泡 UI**：单页面，消息列表 + 底部输入框，类似微信/飞书单聊
2. **语音输入**：调用系统语音输入法（豆包输入法），不内置语音识别
3. **发送 → 写入飞书云文档**：每条消息以 `[HH:MM] 内容` 格式 append 到指定飞书文档
4. **本地消息缓存**：离线时消息存本地，恢复网络后自动同步
5. **秒启动**：冷启动 < 1 秒

## 3. 技术架构

### 3.1 跨平台方案：Tauri 2.0

- **理由**：Rust 后端 + Web 前端，打包体积 < 10MB，启动极快
- **目标平台**：Android（主）、macOS（次）、后续可扩展 iOS/Windows/Linux
- **前端**：纯 HTML/CSS/JS 或 Vue 3（取决于复杂度），无重型框架
- **Rust 侧**：负责飞书 API 调用、token 管理、本地缓存

### 3.2 飞书开放平台 API

#### 3.2.1 认证

- **方式**：企业自建应用 → `tenant_access_token`
- **获取 token**：
  ```
  POST https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal
  Body: { "app_id": "<APP_ID>", "app_secret": "<APP_SECRET>" }
  ```
- **返回**：`{ "tenant_access_token": "xxx", "expire": 7200 }`
- Token 有效期 2 小时，需客户端自动刷新
- **注意**：飞书基础免费版自 2024.10 起限制 10,000 次 API 调用/月，个人使用足够

**参考链接**：
- 自建应用获取 tenant_access_token：https://open.feishu.cn/document/server-docs/authentication-management/access-token/tenant_access_token_internal?lang=zh-CN
- 调用流程概述：https://open.feishu.cn/document/server-docs/api-call-guide/calling-process/overview

#### 3.2.2 核心 API 端点

所有端点 base URL：`https://open.feishu.cn/open-apis`

| 操作 | 方法 | 路径 | 说明 |
|------|------|------|------|
| 创建文档 | POST | `/docx/v1/documents` | 指定标题和文件夹 |
| 获取文档信息 | GET | `/docx/v1/documents/{document_id}` | 获取文档元数据 |
| 获取纯文本 | GET | `/docx/v1/documents/{document_id}/raw_content` | 获取文档全文 |
| 获取块列表 | GET | `/docx/v1/documents/{document_id}/blocks` | 列出所有 block |
| **追加子块** | POST | `/docx/v1/documents/{document_id}/blocks/{block_id}/children` | **核心：追加内容** |
| 更新块 | PATCH | `/docx/v1/documents/{document_id}/blocks/{block_id}` | 更新已有块 |
| 批量更新 | PATCH | `/docx/v1/documents/{document_id}/blocks/batch_update` | 批量操作 |
| Markdown→块 | POST | `/docx/v1/documents/blocks/convert` | 内容格式转换 |
| 删除文档 | DELETE | `/drive/v1/files/{file_token}?type=docx` | 删除文档 |

**频率限制**：单个应用每个 API 上限 3 次/秒。

**参考链接**：
- 文档概述：https://open.feishu.cn/document/server-docs/docs/docs/docx-v1/docx-overview?lang=zh-CN
- 数据结构（Block Types）：https://open.feishu.cn/document/server-docs/docs/docs/docx-v1/docx-structure?lang=zh-CN
- 创建文档：https://open.feishu.cn/document/server-docs/docs/docs/docx-v1/document/create?lang=zh-CN
- 创建块（追加内容）：https://open.feishu.cn/document/server-docs/docs/docs/docx-v1/document-block-children/create?lang=zh-CN
- 更新块：https://open.feishu.cn/document/server-docs/docs/docs/docx-v1/document-block/patch?lang=zh-CN
- 云文档概述：https://open.feishu.cn/document/server-docs/docs/docs-overview?lang=zh-CN

#### 3.2.3 追加内容的具体调用方式

这是 Flash Idea 最核心的 API 调用。每条消息发送时：

```bash
# 向文档的根 block（即 document_id 本身作为 block_id）追加一个文本块
curl -X POST \
  'https://open.feishu.cn/open-apis/docx/v1/documents/{document_id}/blocks/{document_id}/children' \
  -H 'Authorization: Bearer <tenant_access_token>' \
  -H 'Content-Type: application/json' \
  -d '{
    "children": [
      {
        "block_type": 2,
        "text": {
          "elements": [
            {
              "text_run": {
                "content": "[14:32] 明天下午三点和林韬开会讨论视频吊坠的食物识别方案",
                "text_element_style": {}
              }
            }
          ],
          "style": {}
        }
      }
    ]
  }'
```

- `block_type: 2` = 文本段落（Text Block）
- `document_id` 既是文档 ID 也是根 block 的 ID
- 每次追加会在文档末尾新增一个段落

#### 3.2.4 权限配置

在飞书开发者后台（https://open.feishu.cn/app）创建自建应用后，需开通以下权限：
- `docx:document` — 读写文档
- `docx:document:readonly`（可选）— 只读
- `drive:drive` — 访问云空间文件夹

创建后需**发布应用**，权限才生效。

### 3.3 已有工具参考

- **feishu-docs CLI**（OpenClaw 技能）：Node.js CLI 工具，已封装飞书文档 CRUD + Markdown 转换
  - 地址：https://lobehub.com/skills/evan966890-openclaw-bestroll-skills-feishu-docs
  - 用法示例：`node bin/cli.js update -d dcnxxxxxx --append -c "## 补充\n\n新增内容"`
- **@larksuiteoapi/lark-mcp**：飞书官方 MCP Server，支持通过 Claude Code / MCP 客户端操作飞书
  - NPM：`npx -y @larksuiteoapi/lark-mcp mcp -a <app_id> -s <app_secret>`
  - 注意：MCP 模式下文档编辑功能受限（仅支持导入和读取，不支持直接编辑）
- **飞书 API 调试台**：https://open.feishu.cn/api-explorer/ — 在线调试所有 API

## 4. 项目结构

```
flash-idea/
├── README.md                    # 项目说明 + 快速开始
├── LICENSE                      # MIT
├── .env.example                 # 环境变量模板
├── .gitignore
│
├── src-tauri/                   # Tauri Rust 后端
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs              # 入口
│   │   ├── feishu.rs            # 飞书 API 客户端（token管理 + 文档追加）
│   │   ├── cache.rs             # 本地消息缓存（SQLite）
│   │   └── config.rs            # 配置加载
│   └── tauri.conf.json
│
├── src/                         # Web 前端
│   ├── index.html               # 单页面
│   ├── style.css                # 聊天气泡样式
│   └── app.js                   # 消息发送逻辑 + Tauri IPC 调用
│
├── config/
│   └── pipeline.yaml            # [V2] AI 后处理流程定义
│
└── docs/
    ├── setup.md                 # 飞书开放平台配置教程
    └── api-reference.md         # 飞书 API 速查
```

## 5. 环境变量

```env
# .env.example
# 飞书开放平台 - 自建应用凭证
FEISHU_APP_ID=cli_xxxxxxxxxx
FEISHU_APP_SECRET=xxxxxxxxxxxxxxxxxxxxxxxx

# 目标文档 - 消息写入的飞书云文档
FEISHU_DOC_ID=dcnxxxxxxxxxxxxxxxx

# 目标文件夹（用于自动按日期创建新文档，可选）
FEISHU_FOLDER_TOKEN=fldxxxxxxxxxxxxxxxx

# 消息格式
MESSAGE_FORMAT=[{time}] {content}
# 文档策略：append（追加到单文档）或 daily（每天新建文档）
DOC_STRATEGY=append
```

## 6. 用户体验流程

```
用户点击 Flash Idea 图标
    ↓ （< 500ms）
聊天界面展示（显示今日已发送的消息）
    ↓
用户点击输入框 → 豆包输入法弹出 → 切换语音模式 → 说话
    ↓
语音转文字结果填入输入框
    ↓
用户点击发送（或回车）
    ↓
消息显示为气泡 + 异步调用飞书 API 追加到云文档
    ↓
气泡显示 ✓ 同步成功 / ⏳ 等待同步 / ✗ 失败重试
```

## 7. 后续扩展（V2+）

- **AI Pipeline**（`pipeline.yaml`）：每条消息可配置过 LLM 做分类/摘要/提取待办
- **定时汇总**：每天定时将当日所有消息汇总成结构化日报，写入另一个文档
- **多文档路由**：根据关键词或 AI 分类，将消息路由到不同飞书文档
- **Webhook 通知**：重要消息同时推送到飞书群聊机器人
- **飞书录音豆集成**：通过飞书 CLI/API 拉取录音豆的转写文本，统一进入 Flash Idea pipeline

## 8. 开发计划

### Sprint 1（1-2 天）：最小 MVP
- [ ] Tauri 2.0 项目初始化（Android + macOS）
- [ ] 飞书 API 客户端：token 获取 + 自动刷新 + 追加文本块
- [ ] 聊天 UI：消息列表 + 输入框 + 发送按钮
- [ ] 消息发送 → 飞书文档追加 的完整链路

### Sprint 2（1 天）：体验优化
- [ ] 本地 SQLite 缓存 + 离线支持
- [ ] 同步状态指示（✓/⏳/✗）
- [ ] 启动时加载今日已发消息
- [ ] Android 桌面快捷方式优化（指定 Activity 直接启动）

### Sprint 3（可选）：扩展
- [ ] pipeline.yaml 配置系统
- [ ] AI 后处理集成
- [ ] 每日自动创建新文档
