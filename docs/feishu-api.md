# 飞书 API 速查（已于 2026-05-18 联网验证）

## 认证

### 获取 tenant_access_token

```
POST https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal
Content-Type: application/json; charset=utf-8
```

请求体：
```json
{
  "app_id": "<FEISHU_APP_ID>",
  "app_secret": "<FEISHU_APP_SECRET>"
}
```

响应：
```json
{
  "code": 0,
  "msg": "ok",
  "tenant_access_token": "t-xxx",
  "expire": 7200
}
```

- 最大有效期 2 小时
- 剩余有效期 < 30 分钟时调用会返回新 token；>= 30 分钟返回原 token
- 刷新策略：首次发送时 lazy 获取，之后在过期前 30 分钟内主动刷新

参考：https://open.feishu.cn/document/server-docs/authentication-management/access-token/tenant_access_token_internal

## 核心 API：追加文本段落

### 创建块（追加内容）

```
POST https://open.feishu.cn/open-apis/docx/v1/documents/{document_id}/blocks/{block_id}/children
Authorization: Bearer <tenant_access_token>
Content-Type: application/json; charset=utf-8
```

路径参数：
- `document_id`：文档 ID
- `block_id`：父块 ID（追加到文档末尾时填 `document_id`，即根节点）

查询参数：
- `document_revision_id`（可选）：目标文档版本，`-1` 表示最新版（默认）
- `client_token`（可选）：幂等 token，相同 client_token 的重复请求不会重复创建块

请求体（文本段落示例）：
```json
{
  "children": [
    {
      "block_type": 2,
      "text": {
        "elements": [
          {
            "text_run": {
              "content": "[14:32:07] 明天下午三点开会",
              "text_element_style": {}
            }
          }
        ],
        "style": {}
      }
    }
  ]
}
```

- `block_type: 2` = 文本段落（Text）
- `children` 数组最多 50 个块
- `text_element_style` 可传空对象
- 文本内 `\n` 为软换行，新块为硬换行

### 限频

| 层级 | 限制 | 超限响应 |
|------|------|----------|
| 应用级 | 3 次/秒 | HTTP 400 + error 99991400 |
| 单文档级 | 3 并发编辑/秒 | HTTP 429 |

FlashIdea 的同步间隔设为 350ms（约 2.8次/秒），留有余量。

### 幂等性

利用 `client_token` 查询参数实现幂等：每条消息用 message UUID 作为 client_token，崩溃后重试不会在飞书文档中产生重复段落。

## 知识库 Wiki API（文档分割用）

### URL 结构

飞书知识库 URL 格式：`https://xxx.feishu.cn/wiki/{node_token}`

例：`https://my.feishu.cn/wiki/Fxr3wDmpAiTBELkjLdkcgAYOn5u`
- `Fxr3wDmpAiTBELkjLdkcgAYOn5u` 是 **node_token**（知识库节点 token）
- 每个 node 背后挂载一个真实文档，真实文档有自己的 `obj_token`

### 获取节点信息（从 node_token 拿 space_id 和 obj_token）

```
GET /open-apis/wiki/v2/spaces/get_node?token={node_token}
Authorization: Bearer <tenant_access_token>
```

响应：
```json
{
  "code": 0,
  "data": {
    "node": {
      "space_id": "7xxx",
      "node_token": "Fxr3wDmpAiTBELkjLdkcgAYOn5u",
      "obj_token": "dcnXXXXXX",
      "obj_type": "docx",
      "parent_node_token": "...",
      "title": "FlashIdea",
      "has_child": true
    }
  }
}
```

### 在知识库中创建子节点（创建子文档）

```
POST /open-apis/wiki/v2/spaces/{space_id}/nodes
Authorization: Bearer <tenant_access_token>
Content-Type: application/json
```

请求体：
```json
{
  "obj_type": "docx",
  "parent_node_token": "Fxr3wDmpAiTBELkjLdkcgAYOn5u",
  "title": "FlashIdea - 2026-05-19"
}
```

响应：
```json
{
  "code": 0,
  "data": {
    "node": {
      "space_id": "7xxx",
      "node_token": "新node_token",
      "obj_token": "新obj_token（用于 docx API 写内容）",
      "obj_type": "docx",
      "title": "FlashIdea - 2026-05-19"
    }
  }
}
```

### 完整调用流程

1. 从 URL 提取 `node_token`（母文档节点）
2. `GET /wiki/v2/spaces/get_node?token={node_token}` → 拿到 `space_id`
3. `POST /wiki/v2/spaces/{space_id}/nodes` → 创建子文档，拿到 `obj_token`
4. 用 `obj_token` 作为 document_id 调用 docx API 写入内容

## 权限配置

在飞书开发者后台（https://open.feishu.cn/app）创建自建应用后，需开通：
- `docx:document` — 读写文档（必需）
- `wiki:wiki` — 查看、编辑和管理知识库（文档分割必需）

创建后需**发布应用版本**（版本管理与发布 → 创建版本 → 设置可用范围 → 发布），发布后：
- 权限才生效
- 应用才能被搜索到作为协作者
- 在知识库设置中把应用添加为成员，授予编辑权限

## 其他端点（备用）

| 操作 | 方法 | 路径 |
|------|------|------|
| 创建文档 | POST | `/docx/v1/documents` |
| 获取文档信息 | GET | `/docx/v1/documents/{document_id}` |
| 获取 wiki 节点 | GET | `/wiki/v2/spaces/get_node?token={node_token}` |
| 创建 wiki 子节点 | POST | `/wiki/v2/spaces/{space_id}/nodes` |
| 获取纯文本 | GET | `/docx/v1/documents/{document_id}/raw_content` |
| 获取块列表 | GET | `/docx/v1/documents/{document_id}/blocks` |

参考：
- 创建块：https://open.feishu.cn/document/ukTMukTMukTM/uUDN04SN0QjL1QDN/document-docx/docx-v1/document-block-children/create
- 知识库节点：https://open.feishu.cn/document/server-docs/docs/wiki-v2/space-node/create
- 获取节点信息：https://open.feishu.cn/document/server-docs/docs/wiki-v2/space-node/get_node
- API 调试台：https://open.feishu.cn/api-explorer/
