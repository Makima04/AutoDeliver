# AutoDeliver 管理后台模块 — 数据库与 API 契约

> 本文档是新增管理模块（消息中心、自动发货、发货记录、定时任务、操作日志、通知设置、系统设置、管理员、闲鱼连接管理）的**唯一接口契约**。
> 后端与前端都必须以本文档的字段名为准；字段名一旦变化，必须同时更新本文档。

## 1. 总体约定

- 服务：Rust + axum 0.8 + SQLite（sqlx）。
- 所有业务接口前缀 `/api`，JSON 使用 `snake_case`。
- 金额单位：**分**（`price`、`amount` 均为整数分）；前端展示时除以 100。
- 时间：SQLite `DATETIME`，序列化为 `"2026-10-17T18:00:00"`（`chrono::NaiveDateTime`）。
- 分页参数：`limit`（默认 50，最大 200）、`offset`（默认 0）。
- 错误响应统一为：`{ "code": "INVALID_INPUT", "message": "..." }`。
- 登录态：`Authorization: Bearer <token>`。新增管理接口使用 `handlers::auth::require_admin(&state, &headers)` 校验；前端登录后把 token 存入 `localStorage.adm_token`，并在 `apiFetch` 中自动附加。
- 除 `/api/auth/login`、`/api/webhooks/*`、`/` 与静态资源外，所有新增管理接口都要求管理员登录。
- 所有新增 handler 模块必须提供 `pub fn router() -> Router<AppState>`，由 `src/handlers.rs` 统一 `.merge(...)`。
- 所有新增 db 模块使用 `sqlx::query_as::<_, T>` + `#[derive(sqlx::FromRow)]`，禁止 `mod.rs`。

## 2. 数据库表（migrations/002_admin_modules.sql 已创建）

### 2.1 admins / admin_sessions / admin_login_logs

```sql
admins(id, username UNIQUE, password_hash, display_name, role, status,
       last_login_at, last_login_ip, created_at, updated_at)
-- role: super_admin | admin | viewer
-- status: active | disabled

admin_sessions(token PK, admin_id, username, role, expires_at, created_at)

admin_login_logs(id, admin_id, username, ip, user_agent, success, message, created_at)
```

- 密码存储格式：`sha256(salt + ":" + password)`，落库为 `"<salt>$<hex>"`。
- 首次启动时 `db::admin::ensure_seed_admin(pool, &AuthConfig)` 用 `config.toml [auth]` 的账号密码创建 `super_admin`（若表为空）。
- 登录优先查 `admins`；DB 中没有该用户名时回退校验 `config.auth`（兼容旧配置），成功后也签发 session。

### 2.2 xianyu_connection_logs

```sql
xianyu_connection_logs(id, account_id, event, status, message, created_at)
-- event: login | qr_login | verify | reconnect | disconnect | delete | sync | error
-- status: success | failed | info
```

### 2.3 delivery_records

```sql
delivery_records(
  id, order_id, account_id, xianyu_item_id, platform_order_id,
  product_id, product_name, buyer_nickname, delivery_content,
  status, source, remote_consign_status, remote_im_status,
  error_message, duration_ms, created_at
)
-- status: success | failed | pending
-- source: auto_scan | webhook | manual | api | retry
```

### 2.4 message_conversations / messages

```sql
message_conversations(
  id, account_id, seller_nick, session_cid, buyer_id, buyer_nickname,
  xianyu_item_id, last_message, last_message_at, unread_count,
  status, created_at, updated_at,
  UNIQUE(account_id, session_cid)
)
-- status: open | closed

messages(
  id, conversation_id, account_id, session_cid, direction, sender_id,
  sender_nick, content, msg_type, order_id, remote_msg_id, created_at
)
-- direction: in | out
-- msg_type: text | order_card | system
```

### 2.5 scheduled_tasks / scheduled_task_runs

```sql
scheduled_tasks(
  id, task_key UNIQUE, name, kind, interval_seconds, enabled,
  last_run_at, next_run_at, last_status, last_message, last_duration_ms,
  run_count, fail_count, created_at, updated_at
)
-- last_status: idle | running | success | failed
-- kind: auto_deliver_scan | item_sync | message_sync | account_health |
--       cleanup_logs | delivery_retry | notification_retry

scheduled_task_runs(
  id, task_id, task_key, status, message, duration_ms, started_at, finished_at
)
```

内置任务：

| task_key | kind | interval | 默认启用 |
|---|---|---|---|
| `xianyu_auto_deliver` | auto_deliver_scan | 45s | 是 |
| `xianyu_item_sync` | item_sync | 300s | 是 |
| `xianyu_message_sync` | message_sync | 60s | 是 |
| `xianyu_account_health` | account_health | 1800s | 否 |
| `system_cleanup` | cleanup_logs | 86400s | 是 |
| `delivery_retry` | delivery_retry | 120s | 否 |
| `notification_retry` | notification_retry | 300s | 是 |

### 2.6 operation_logs

```sql
operation_logs(
  id, actor, action, method, path, resource_type, resource_id,
  status_code, detail, ip, user_agent, duration_ms, created_at
)
```

- `src/audit.rs` 的中间件自动记录所有 `POST/PUT/PATCH/DELETE /api/*`（排除 `/api/webhooks/*`），`detail` 默认空。
- 关键操作可额外调用 `audit::record(...)` 写入可读 detail。

### 2.7 notification_channels / notification_rules / notification_logs

```sql
notification_channels(id, name, channel_type, config_json, enabled, created_at, updated_at)
-- channel_type: webhook | feishu | serverchan | email | sms

notification_rules(id, channel_id, event_type, enabled, template, created_at, updated_at,
                   UNIQUE(channel_id, event_type))

notification_logs(id, channel_id, event_type, title, content, status,
                  response, error_message, created_at)
-- status: sent | failed | pending
```

事件类型：`delivery_success`、`delivery_failed`、`account_offline`、`out_of_stock`、`task_failed`、`order_refunded`、`system_alert`。

模板占位符：`{event}`、`{title}`、`{content}`、`{time}`。

### 2.8 system_settings

```sql
system_settings(key PK, value, label, group_name, description, value_type, updated_at)
-- value_type: string | int | bool | json
```

默认键（`db::system::ensure_defaults` 负责补齐）：

| key | 默认值 |
|---|---|
| `site_name` | `AutoDeliver 自动发货平台` |
| `site_description` | `闲鱼多账号自动发货与消息管理` |
| `timezone` | `Asia/Shanghai` |
| `default_page` | `products` |
| `maintenance_mode` | `false` |
| `session_timeout_minutes` | `720` |
| `password_min_length` | `8` |
| `log_retention_days` | `30` |
| `delivery_retention_days` | `90` |
| `notification_retention_days` | `30` |
| `auto_sync_on_startup` | `true` |
| `auto_delivery_enabled` | `true` |
| `auto_delivery_scan_interval` | `45` |
| `auto_delivery_auto_consign` | `true` |
| `auto_delivery_auto_send_message` | `true` |
| `auto_delivery_reply_template` | 配置里的闲鱼回复模板 |
| `auto_delivery_retry_limit` | `3` |
| `auto_delivery_low_stock_threshold` | `5` |
| `auto_delivery_notify_on_success` | `false` |
| `auto_delivery_notify_on_failure` | `true` |
| `auto_delivery_dry_run` | `false` |
| `message_sync_enabled` | `true` |
| `message_auto_reply_enabled` | `false` |
| `message_auto_reply_template` | `您好，已收到您的消息，稍后回复您。` |

### 2.9 orders 扩展列（由 db::init 自动 ALTER）

```sql
order_status TEXT NOT NULL DEFAULT 'delivered'   -- pending|paid|delivering|delivered|failed|refunded|closed
delivery_status TEXT NOT NULL DEFAULT 'success'  -- pending|success|failed
amount INTEGER NOT NULL DEFAULT 0                -- 分
account_id INTEGER
xianyu_item_id TEXT
updated_at DATETIME
```

## 3. API 契约

### 3.1 认证与管理员

#### POST /api/auth/login
请求：`{ "username": "admin", "password": "admin123" }`

响应：
```json
{
  "authenticated": true,
  "username": "admin",
  "display_name": "超级管理员",
  "role": "super_admin",
  "admin_id": 1,
  "token": "ad_xxx",
  "expires_at": "2026-10-18T18:00:00"
}
```

失败：401 `{ "code": "UNAUTHORIZED", "message": "管理员账号或密码错误" }`。
登录成功/失败都写 `admin_login_logs`。

#### GET /api/auth/me
Header 必须带 Bearer token。响应：
```json
{ "authenticated": true, "username": "admin", "display_name": "超级管理员", "role": "super_admin", "admin_id": 1, "expires_at": "..." }
```

#### POST /api/auth/logout
使当前 token 失效，响应 `{ "ok": true }`。

#### POST /api/auth/change-password
请求 `{ "old_password": "...", "new_password": "..." }`，响应 `{ "ok": true, "message": "密码已更新" }`。

#### GET /api/admins
响应：`[{ "id":1, "username":"admin", "display_name":"超级管理员", "role":"super_admin", "status":"active", "last_login_at":null, "last_login_ip":null, "created_at":"..." }]`

#### POST /api/admins
请求 `{ "username":"ops", "password":"******", "display_name":"运营", "role":"admin", "status":"active" }`，响应同上单个对象。

#### PUT /api/admins/{id}
请求 `{ "display_name":"...", "role":"admin", "status":"active", "password": null }`（`password` 为空表示不修改）。

#### DELETE /api/admins/{id}
响应 `{ "ok": true }`；禁止删除最后一个 `super_admin` 和当前登录账号。

#### GET /api/admins/login-logs?limit=50
响应：`[{ "id":1, "admin_id":1, "username":"admin", "ip":"127.0.0.1", "user_agent":"...", "success":true, "message":"登录成功", "created_at":"..." }]`

### 3.2 闲鱼账号连接管理

保留现有接口：
- `GET /api/xianyu/accounts`
- `POST /api/xianyu/qrcode/generate`、`POST /api/xianyu/qrcode/poll`
- `POST /api/xianyu/login`、`POST /api/xianyu/verify`
- `POST /api/xianyu/accounts/{id}/verify`
- `DELETE /api/xianyu/accounts/{id}`

新增：
#### GET /api/xianyu/accounts/{id}/connection-logs?limit=50
响应：`[{ "id":1, "account_id":1, "event":"verify", "status":"success", "message":"远端验签成功", "created_at":"..." }]`

#### POST /api/xianyu/accounts/{id}/reconnect
执行远端验签并刷新 cookie/token，写连接日志。响应：
```json
{ "account_id": 1, "connected": true, "valid": true, "message": "重连成功", "remote_response_preview": "..." }
```

#### GET /api/xianyu/accounts/health
响应：
```json
[{ "account_id":1, "seller_nick":"...", "status":"connected", "last_verified_at":"...",
   "items_count":12, "order_count":3, "last_connection_event":"verify",
   "last_connection_status":"success", "last_error":null }]
```

### 3.3 商品管理

保留现有 `/api/products*`、`/api/xianyu/items*`。

新增：
#### GET /api/products/stats
```json
{ "total":10, "enabled":8, "disabled":2, "total_codes":100, "unused_codes":60,
  "sold_codes":40, "low_stock_count":2, "bound_items":5 }
```

#### DELETE /api/codes/id/{id}
删除单条兑换码，响应 `{ "ok": true, "message": "..." }`。

### 3.4 订单管理

#### GET /api/orders
保持数组，新增字段：`order_status`、`delivery_status`、`amount`、`account_id`、`xianyu_item_id`、`updated_at`。

#### GET /api/orders/stats
```json
{ "total":20, "today":5, "pending":1, "delivered":18, "failed":1,
  "refunded":0, "revenue":19900, "today_revenue":5000 }
```

#### POST /api/orders/sync
从闲鱼卖家订单列表（`mtop.taobao.idle.trade.merchant.sold.get`）同步待发货订单。
只入库，不抽卡密、不调用远端发货。响应：
```json
{
  "accounts": 1,
  "remote_total": 1,
  "fetched": 1,
  "imported": 1,
  "skipped_existing": 0,
  "unbound": 0,
  "errors": [],
  "message": "订单同步完成：远端待发货 1 笔，检查 1 笔，新导入 1 笔，已存在 0 笔，未绑定商品 0 笔"
}
```

#### PATCH /api/orders/{id}/status
请求 `{ "order_status": "delivered", "delivery_status": "success" }`（两者都可选），响应订单详情。

#### POST /api/orders/{id}/retry
重新执行该订单的闲鱼虚拟发货 + IM 发送（仅闲鱼订单）。响应：
```json
{ "ok": true, "message": "重试完成", "order": { }, "delivery_record_id": 12 }
```

#### POST /api/orders/{id}/resend
仅通过 IM 重发已发货内容。响应 `{ "ok": true, "message": "...", "remote_ret": "..." }`。

#### DELETE /api/orders/{id}
响应 `{ "ok": true, "message": "..." }`。

### 3.5 在线消息

#### GET /api/messages/conversations?account_id=&search=&status=&limit=&offset=
```json
[{
  "id": 1, "account_id": 1, "seller_nick": "卖家",
  "session_cid": "55889911@goofish", "buyer_id": "220011", "buyer_nickname": "买家",
  "xianyu_item_id": "1050090004196", "last_message": "在吗？",
  "last_message_at": "2026-10-17T18:00:00", "unread_count": 2,
  "status": "open", "created_at": "...", "updated_at": "..."
}]
```

#### GET /api/messages/stats
`{ "conversations": 12, "unread": 3, "today_messages": 8, "open_conversations": 10 }`

#### POST /api/messages/sync
请求 `{ "account_id": 1 }`（可空，空表示全部账号）。响应：
```json
{ "synced_accounts": 1, "synced_conversations": 12, "synced_messages": 12,
  "errors": [], "message": "已同步 1 个账号的 12 个会话" }
```

#### GET /api/messages/conversations/{id}
```json
{ "conversation": { ... }, "messages": [
  { "id":1, "conversation_id":1, "account_id":1, "session_cid":"...",
    "direction":"in", "sender_id":"220011", "sender_nick":"买家",
    "content":"在吗？", "msg_type":"text", "order_id":null,
    "remote_msg_id":null, "created_at":"..." }
] }
```

#### POST /api/messages/conversations/{id}/send
请求 `{ "content": "您好，在的" }`。调用闲鱼 IM 发送；无论远端是否成功都把 out 消息落库，并在响应里说明：
```json
{ "message": { ... }, "remote_sent": true, "remote_ret": "SUCCESS::调用成功",
  "message_text": "消息已发送" }
```

#### POST /api/messages/conversations/{id}/read
响应 `{ "ok": true, "unread_count": 0 }`。

#### DELETE /api/messages/conversations/{id}
响应 `{ "ok": true }`。

### 3.6 自动发货

#### GET /api/auto-delivery/config
```json
{ "enabled": true, "scan_interval_seconds": 45, "auto_consign": true,
  "auto_send_message": true, "reply_template": "【自动发货】...",
  "retry_limit": 3, "low_stock_threshold": 5,
  "notify_on_success": false, "notify_on_failure": true, "dry_run": false }
```

#### PUT /api/auto-delivery/config
请求同结构（字段均可选，后端按现有值合并），响应同 GET。
更新 `scan_interval_seconds` 时同步更新 `scheduled_tasks.task_key='xianyu_auto_deliver'` 的 `interval_seconds`。

#### GET /api/auto-delivery/readiness
```json
[{
  "product_id": 1, "product_name": "50元卡密", "enabled": true,
  "template_type": "redeem_code", "unused_codes": 20,
  "xianyu_item_id": "1050090004196", "xianyu_title": "商品标题",
  "account_id": 1, "account_nick": "卖家", "item_status": "on_sale",
  "ready": true, "issue": null
}]
```
`ready=false` 时 `issue` 给出原因（未绑定、已下架、库存不足、商品禁用、账号异常）。

#### POST /api/auto-delivery/scan
请求 `{ "account_id": 1 }`（可空）。复用现有扫描逻辑，响应在 `AutoDeliverScanResponse` 基础上增加：
```json
{ ...现有字段..., "delivery_summary": { "succeeded": 2, "failed": 0, "records": 2 } }
```

#### GET /api/auto-delivery/stats
`{ "today_delivered": 10, "today_failed": 1, "success_rate": 90.9, "pending_orders": 2, "unused_codes": 100 }`

### 3.7 发货记录

#### GET /api/deliveries?status=&account_id=&search=&limit=&offset=
```json
{ "items": [{
  "id": 1, "order_id": 1, "account_id": 1, "xianyu_item_id": "105...",
  "platform_order_id": "4123...", "product_id": 1, "product_name": "50元卡密",
  "buyer_nickname": "买家", "delivery_content": "CARD-XXXX",
  "status": "success", "source": "auto_scan",
  "remote_consign_status": "SUCCESS", "remote_im_status": "SUCCESS",
  "error_message": null, "duration_ms": 1234, "created_at": "..."
}], "total": 20, "limit": 50, "offset": 0 }
```

#### GET /api/deliveries/stats
`{ "total": 100, "success": 95, "failed": 5, "today": 10, "avg_duration_ms": 1200, "success_rate": 95.0 }`

#### POST /api/deliveries/{id}/retry
按记录里的 `platform_order_id` / `xianyu_item_id` / `product_id` 重新执行发货，响应 `{ "ok": true, "message": "...", "record": { } }`。

#### DELETE /api/deliveries/{id}
`{ "ok": true }`

#### GET /api/deliveries/export
返回 CSV（`text/csv`），列：ID,订单ID,平台单号,商品,买家,状态,来源,发货内容,错误,耗时ms,时间。

### 3.8 定时任务

#### GET /api/scheduler/tasks
```json
[{ "id":1, "task_key":"xianyu_auto_deliver", "name":"闲鱼自动发货扫描",
   "kind":"auto_deliver_scan", "interval_seconds":45, "enabled":true,
   "last_run_at":"...", "next_run_at":"...", "last_status":"success",
   "last_message":"...", "last_duration_ms":1200, "run_count":10,
   "fail_count":0, "created_at":"...", "updated_at":"..." }]
```

#### POST /api/scheduler/tasks
请求 `{ "task_key":"my_task", "name":"...", "kind":"item_sync", "interval_seconds":600, "enabled":true }`。

#### PUT /api/scheduler/tasks/{id}
请求 `{ "name":"...", "interval_seconds":600, "enabled":true }`（均可选）。

#### DELETE /api/scheduler/tasks/{id}
`{ "ok": true }`；内置任务也允许删除，但前端给出确认。

#### POST /api/scheduler/tasks/{id}/run
立即执行一次。响应 `{ "ok": true, "message": "...", "task": { } }`。

#### GET /api/scheduler/runs?task_id=&limit=50
`[{ "id":1, "task_id":1, "task_key":"...", "status":"success", "message":"...", "duration_ms":1200, "started_at":"...", "finished_at":"..." }]`

### 3.9 操作日志

#### GET /api/logs?search=&method=&status=&from=&to=&limit=&offset=
```json
{ "items": [{ "id":1, "actor":"admin", "action":"POST /api/products",
  "method":"POST", "path":"/api/products", "resource_type":"products",
  "resource_id":null, "status_code":201, "detail":"创建商品：50元卡密",
  "ip":"127.0.0.1", "user_agent":"...", "duration_ms":12, "created_at":"..." }],
  "total": 100, "limit": 50, "offset": 0 }
```

#### GET /api/logs/stats
`{ "total":100, "today":20, "failed":2, "top_paths":[{"path":"/api/products","count":10}] }`

#### DELETE /api/logs/{id}
`{ "ok": true }`

#### DELETE /api/logs?before_days=30
清理 N 天前日志，响应 `{ "ok": true, "deleted": 20 }`。

#### GET /api/logs/export
CSV 导出。

### 3.10 通知设置

#### GET /api/notifications/channels
`[{ "id":1, "name":"默认 Webhook", "channel_type":"webhook",
    "config_json":"{\"url\":\"\"}", "enabled":false,
    "created_at":"...", "updated_at":"..." }]`

#### POST /api/notifications/channels
请求 `{ "name":"...", "channel_type":"webhook", "config_json":"{...}", "enabled":true }`。

#### PUT /api/notifications/channels/{id}
同 POST 字段（均可选）。

#### DELETE /api/notifications/channels/{id}
`{ "ok": true }`

#### POST /api/notifications/channels/{id}/test
发送测试通知。响应 `{ "ok": true, "status": "sent", "response": "...", "error_message": null }`。

#### GET /api/notifications/rules
`[{ "id":1, "channel_id":1, "channel_name":"默认 Webhook", "event_type":"delivery_failed",
    "enabled":true, "template":"...", "created_at":"...", "updated_at":"..." }]`

#### POST /api/notifications/rules
`{ "channel_id":1, "event_type":"delivery_failed", "enabled":true, "template":"..." }`

#### PUT /api/notifications/rules/{id}
`{ "enabled":true, "template":"..." }`

#### DELETE /api/notifications/rules/{id}
`{ "ok": true }`

#### GET /api/notifications/logs?limit=50&status=
`[{ "id":1, "channel_id":1, "channel_name":"...", "event_type":"delivery_failed",
    "title":"...", "content":"...", "status":"failed", "response":null,
    "error_message":"...", "created_at":"..." }]`

#### POST /api/notifications/logs/{id}/retry
`{ "ok": true, "status": "sent", "message": "..." }`

#### GET /api/notifications/events
`[{ "event_type":"delivery_success", "label":"发货成功" }, ...]`

### 3.11 系统设置

#### GET /api/settings
`[{ "key":"site_name", "value":"...", "label":"站点名称", "group_name":"basic",
    "description":"...", "value_type":"string", "updated_at":"..." }]`

#### PUT /api/settings
请求 `{ "settings": { "site_name": "AutoDeliver", "log_retention_days": "30" } }`，响应更新后的完整数组。

#### GET /api/system/info
```json
{ "version":"0.1.0", "uptime_seconds":1234, "db_size_bytes":204800,
  "timezone":"Asia/Shanghai", "server_time":"2026-10-17T18:00:00",
  "counts": { "accounts":1, "items":12, "products":5, "orders":20,
              "conversations":3, "deliveries":20, "scheduled_tasks":7,
              "unread_messages":2 } }
```

#### POST /api/system/backup
备份 SQLite 文件到 `data/backups/`，响应 `{ "ok": true, "file": "data/backups/auto-delivery-20261017-180000.db", "size": 204800, "message": "..." }`。

#### GET /api/system/backups
`[{ "file":"...", "size":204800, "created_at":"..." }]`

#### POST /api/system/cleanup
请求 `{ "days": 30 }`，按保留策略清理操作日志、通知日志、发货记录，响应 `{ "ok": true, "deleted_logs":10, "deleted_notifications":2, "deleted_deliveries":0 }`。

## 4. 前端页面契约

在 `static/index.html` 左侧导航新增分组：

- **运营**：商品（现有）、卡密库存（现有）、订单（现有）、在线消息（新）、发货记录（新）
- **闲鱼**：闲鱼账号（现有）、搜索分析（现有）、自动发货（新）
- **系统**：定时任务（新）、操作日志（新）、通知设置（新）、系统设置（新）、管理员（新）

新增 section 容器（放在 `static/index.html` 的 `main` 内）：
`view-messages`、`view-autodeliver`、`view-deliveries`、`view-scheduler`、`view-logs`、
`view-notifications`、`view-system`、`view-admins`。

页面由两个独立脚本渐进增强，脚本只写自己的文件：
- `static/modules-ops.js`：在线消息、自动发货、发货记录
- `static/modules-system.js`：定时任务、操作日志、通知设置、系统设置、管理员

注册方式：
```js
window.pageLoaders = window.pageLoaders || {};
window.pageLoaders['messages'] = loadMessagesPage;
```
`switchTab(tab)` 在切换后调用 `window.pageLoaders?.[tab]?.()`。

`apiFetch` 自动附加 `Authorization: Bearer localStorage.adm_token`；收到 401 时清除 token 并提示登录。

## 5. 模块文件归属（并行开发约定）

| 范围 | 文件 |
|---|---|
| 共享契约 / 迁移 / 路由聚合 / 订单扩展 | `migrations/002_admin_modules.sql`、`src/db.rs`、`src/handlers.rs`、`src/main.rs`、`src/db/order.rs`、`src/db/product.rs`、`src/handlers/order.rs`、`src/models/order.rs` |
| 定时任务 / 系统设置 / 系统信息 / 操作日志 | `src/db/scheduler.rs`、`src/handlers/scheduler.rs`、`src/scheduler.rs`、`src/db/system.rs`、`src/handlers/system.rs`、`src/audit.rs` |
| 通知 / 发货记录 / 自动发货 | `src/db/notification.rs`、`src/handlers/notification.rs`、`src/notify.rs`、`src/db/delivery.rs`、`src/handlers/delivery.rs`、`src/handlers/auto_delivery.rs` |
| 在线消息 | `src/db/message.rs`、`src/handlers/message.rs`、`src/platform/xianyu.rs`（仅追加会话抓取函数） |
| 管理员 / 闲鱼连接日志 | `src/db/admin.rs`、`src/handlers/admin.rs`、`src/handlers/auth.rs`、`src/db/xianyu_connection.rs`、`src/handlers/account.rs` |
| 前端 | `static/modules-ops.js`、`static/modules-system.js`、`static/index.html`（仅壳层） |

任何跨文件新增公开函数，必须先在本文件或对应 stub 中固定签名。
