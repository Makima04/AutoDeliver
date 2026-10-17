# AutoDeliver 同步与自动化机制

> 本文说明 AutoDeliver 中「数据从哪里来、什么时候同步、如何合并、失败会怎样」。
> 内容以当前代码为准，涉及的主要实现文件：
>
> - 调度器：[`src/scheduler.rs`](../src/scheduler.rs)
> - 商品同步：[`src/platform/xianyu_market.rs`](../src/platform/xianyu_market.rs)、[`src/db/xianyu.rs`](../src/db/xianyu.rs)
> - 订单同步：[`src/order_sync.rs`](../src/order_sync.rs)、[`src/platform/xianyu_trade.rs`](../src/platform/xianyu_trade.rs)
> - 消息同步：[`src/platform/xianyu.rs`](../src/platform/xianyu.rs)、[`src/db/message.rs`](../src/db/message.rs)
> - 自动发货：[`src/handlers/xianyu.rs`](../src/handlers/xianyu.rs)
> - 发货记录 / 通知：[`src/handlers/delivery.rs`](../src/handlers/delivery.rs)、[`src/notify.rs`](../src/notify.rs)
> - 数据库结构：[`migrations/002_admin_modules.sql`](../migrations/002_admin_modules.sql)

---

## 1. 同步总览

AutoDeliver 没有一个“万能同步按钮”，而是按数据类型拆成多条独立链路：

| 数据流 | 方向 | 触发方式 | 默认频率 | 核心入口 |
|---|---|---|---|---|
| 闲鱼账号凭证刷新 | 远端 → 本地 | 任意远端调用成功后 | 随调用 | `update_account_refreshed_credentials` |
| 闲鱼商品同步 | 远端 → 本地 | 页面自动、手动、定时任务、自动发货扫描 | 15s / 手动 / 300s / 45s | `sync_items_for_account` |
| 商品编辑/改价/上下架 | 本地 → 远端 | 用户在「商品」页操作 | 按需 | `update_item`、`update_item_price` |
| 在线消息同步 | 远端 → 本地 | 定时任务、手动「同步消息」 | 60s / 手动 | `sync_all_accounts` |
| 消息自动回复 | 本地 → 远端 | 消息同步发现新消息后 | 随消息同步 | `sync_all_accounts` |
| 手动回复买家 | 本地 → 远端 | 用户在「在线消息」页发送 | 按需 | `send_message` |
| 待发货订单同步 | 远端 → 本地 | 定时任务、订单页加载、手动按钮 | 60s / 按需 | `order_sync::sync_all_accounts` |
| IM 待发货卡片扫描 | 远端 → 本地 | 自动发货定时任务、手动扫描 | 45s / 手动 | `run_auto_deliver_scan_cycle` |
| 自动发货 | 本地 + 远端 | 扫描发现待发货订单后 | 随扫描 | `fulfill_xianyu_order_with_remote_consign` |
| 发货记录 | 本地写入 | 每次发货尝试后 | 随发货 | `delivery_records` |
| 通知 | 本地 → 外部 | 发货成功/失败后按规则 | 随事件 | `notify::dispatch_event` |
| 操作日志 | 本地写入 | 任意变更类 API 请求 | 随请求 | `audit` 中间件 |
| 连接日志 | 本地写入 | 登录、验签、重连、健康检查 | 随事件 | `xianyu_connection_logs` |

一句话概括：

- **商品和消息是“远端 → 本地”的镜像式同步**；
- **订单以待发货列表同步为主通道**（`mtop.taobao.idle.trade.merchant.sold.get`），IM 会话扫描只作为有聊天记录订单的补充通道；
- **订单同步只导入待发货订单，不自动发货**；自动发货是独立的履约动作，由 IM 扫描或用户点击订单行「发货」/「重试」触发；
- **所有远端成功响应都会顺手刷新账号 Cookie / Token，避免登录态越用越旧**。

---

## 2. 数据表与职责

| 表 | 作用 | 谁写入 |
|---|---|---|
| `xianyu_accounts` | 闲鱼账号、Cookie、`_m_h5_tk`、OpenAPI 凭证 | 登录、验签、远端调用后的凭证刷新 |
| `xianyu_items` | 闲鱼商品本地镜像，含绑定卡密商品 `local_product_id` | 商品同步、手动新增/编辑 |
| `products` | 本地卡密商品及发货模板 | 卡密库存中心 |
| `redeem_codes` | 兑换码库存 | 批量导入、发货时扣减 |
| `orders` | 订单记录：订单同步导入的待发货单，或已分配发货内容的订单 | 订单同步 / 自动发货 / 手动下单 / Webhook |
| `delivery_records` | 每次发货尝试的审计流水 | 自动发货包装函数、手动下单 |
| `message_conversations` | 闲鱼 IM 会话 | 消息同步 |
| `messages` | 会话中的入站/出站消息 | 消息同步、手动回复、自动回复 |
| `scheduled_tasks` | 定时任务定义与运行状态 | 启动初始化、管理后台 |
| `scheduled_task_runs` | 每次任务执行历史 | 调度器 |
| `operation_logs` | 管理端变更接口审计 | `audit` 中间件 |
| `xianyu_connection_logs` | 账号连接事件 | 登录、验签、重连、健康检查 |
| `notification_*` | 通知渠道、规则、发送日志 | 通知设置页、事件分发 |
| `system_settings` | 自动发货、消息、安全、保留策略等配置 | 系统设置 / 自动发货页 |

---

## 3. 调度器：所有后台同步的心脏

### 3.1 启动与循环

服务启动后：

1. `db::init` 执行迁移；
2. `ensure_default_tasks` 幂等插入 8 个内置任务（旧库会自动补齐新增的 `xianyu_order_sync`）；
3. `scheduler::run_loop` 启动后台循环；
4. 若 `config.xianyu.enabled = true`，服务启动 15 秒后额外触发一次自动发货扫描（`main.rs` 的 warm-up）；
5. 内置任务初始化时 `next_run_at = CURRENT_TIMESTAMP`，因此**首次启动后会立即执行一轮到期任务**；之后按“上次完成时间 + 间隔”计算。

调度循环逻辑：

```text
每 5 秒
  → 查询 enabled=1 且 next_run_at <= 当前时间的任务（按 id 升序）
  → 逐个串行执行
  → begin_run：last_status 从非 running 改为 running（防并发）
  → execute_task(kind)
  → finish_run：写 scheduled_task_runs，更新 last_status / run_count / fail_count /
                last_message / next_run_at = now + interval
  → 单个任务失败不会中断循环
```

关键细节：

- **串行执行**：任务之间不并发；一个慢任务会推迟后续任务。
- **最小间隔 10 秒**：`interval_seconds` 会被 `effective_interval_seconds()` 钳制到 ≥10。
- **防重复执行**：`begin_run` 用 `WHERE last_status <> 'running'` 条件更新；重复点击/循环撞车会返回「任务正在执行中」。
- **异常恢复**：调度器启动时会把残留的 `running` 状态复位为 `idle`，避免进程崩溃后任务永久卡死。
- **手动执行不受 `enabled` 影响**：`run_task_now` 只检查任务是否存在、是否正在运行。
- **任务执行历史**保留在 `scheduled_task_runs`，系统清理任务会删除 7 天前的记录。

### 3.2 内置任务

| task_key | kind | 默认间隔 | 默认启用 | 作用 |
|---|---|---:|---|---|
| `xianyu_auto_deliver` | `auto_deliver_scan` | 45s | 是 | 同步商品 + 扫描待发货会话 + 自动发货 |
| `xianyu_item_sync` | `item_sync` | 300s | 是 | 全账号商品同步 |
| `xianyu_order_sync` | `order_sync` | 60s | 是 | 全账号待发货订单导入（只入库，不自动发货） |
| `xianyu_message_sync` | `message_sync` | 60s | 是 | 全账号消息会话同步 |
| `xianyu_account_health` | `account_health` | 1800s | 否 | 全账号远端验签 + 连接日志 |
| `system_cleanup` | `cleanup_logs` | 86400s | 是 | 按保留天数清理日志/记录 |
| `delivery_retry` | `delivery_retry` | 120s | 否 | 重试最近 7 天失败发货 |
| `notification_retry` | `notification_retry` | 300s | 是 | 重试 pending 通知 |

### 3.3 管理入口

- 页面：「定时任务」
- API：
  - `GET /api/scheduler/tasks`
  - `PUT /api/scheduler/tasks/{id}`
  - `POST /api/scheduler/tasks/{id}/run`
  - `GET /api/scheduler/runs?limit=50`

注意：在「自动发货」页保存扫描间隔时，后端会同步更新 `xianyu_auto_deliver.interval_seconds` 并把 `next_run_at` 设为当前时间；如果直接在「定时任务」页改同一个任务的间隔，则以任务表为准。

---

## 4. 闲鱼账号连接与凭证刷新

### 4.1 三种接入方式

| 方式 | 入口 | 保存内容 | 后续调用 |
|---|---|---|---|
| 手机扫码 | `POST /api/xianyu/qrcode/generate` + `poll` | 扫码确认后解析出的 Cookie、`_m_h5_tk`、卖家 ID/昵称 | 与 Cookie 模式相同 |
| Cookie | `POST /api/xianyu/login` | 原始 Cookie、`_m_h5_tk`、卖家 ID/昵称 | MTOP H5 |
| 闲鱼管家 OpenAPI | `POST /api/xianyu/login`（`auth_mode=open_api`） | AppKey、AppSecret、AccessToken | `open.goofish.com` |

### 4.2 登录后的验签

登录成功后会立即调用 `verify_remote_account`：

1. 请求闲鱼用户信息接口；
2. 解析真实数字卖家 ID 与昵称；
3. 刷新 Cookie / `_m_h5_tk`；
4. 更新 `xianyu_accounts.status`、`last_verified_at`；
5. 写入 `xianyu_connection_logs`。

### 4.3 凭证持续刷新

所有会访问闲鱼的链路，在成功后都会调用：

```rust
db::xianyu::update_account_refreshed_credentials(
    pool, account_id,
    refreshed_cookie,
    refreshed_token,
    resolved_seller_id,
    resolved_seller_nick,
)
```

因此：

- 闲鱼轮换 `_m_h5_tk` 后，下一次同步会使用新值；
- Cookie 中的 `cna`、登录态变化会被持久化；
- 商品同步、消息同步、自动发货扫描、健康检查都会刷新凭证；
- 连接日志会记录登录、验签、重连、健康检查等事件。

### 4.4 账号健康检查

由 `xianyu_account_health` 任务触发（默认关闭，间隔 1800 秒）：

1. 遍历全部账号；
2. 调用 `verify_remote_account`；
3. 成功时刷新 Cookie / Token / 卖家昵称；
4. `mark_account_verified_by_id` 更新账号状态与最后校验时间；
5. 写 `xianyu_connection_logs`（`event=verify`，`success/failed`）；
6. 单个账号失败不会中断其他账号。

该任务关闭时，仍会在登录、商品同步、消息同步、自动发货扫描时被动刷新凭证并更新状态。

---

## 5. 商品同步（远端 → 本地）

### 5.1 远端拉取

入口：`platform::xianyu_market::fetch_remote_items_for_account`。

#### Cookie / H5 模式

1. 读取 `raw_cookie`；
2. 如果 `seller_id` 不是纯数字，先调用：
   - `mtop.idle.web.user.page.nav`，失败再回退 `mtop.taobao.idlemessage.pc.loginuser.get`；
   - 解析真实卖家 ID 与昵称；
3. 分页调用 `mtop.idle.web.xyh.item.list`：
   - `pageNumber` 从 1 开始；
   - 每页 `pageSize=20`；
   - 最多请求 3 页；
   - 每页去重后合并；
   - 当远端 `nextPage=false` 或本页没有新增商品时停止；
4. 每页请求后刷新 Cookie / Token；
5. 返回 `RemoteAccountSyncOutcome`：
   - `items`
   - `computed_sign`
   - `remote_status`（远端 `ret` 或友好错误）
   - `refreshed_cookie` / `refreshed_token`
   - `resolved_seller_id` / `resolved_seller_nick`

#### OpenAPI 模式

1. 校验 AppKey / AppSecret；
2. 生成签名；
3. 调用 `GET /api/open/product/list`，`page_size=50`；
4. 解析商品列表；
5. 返回同样的 `RemoteAccountSyncOutcome`。

### 5.2 本地合并

入口：`db::xianyu::sync_items_for_account`，整个合并过程在**一个事务**内完成。

核心算法：

```text
1. 收集本次远端返回的所有 xianyu_item_id（remote_id_set）；
2. 如果远端列表非空：
     把该账号下“数字开头 ID”的本地商品全部先标记为 off_shelf，
     synced_at = 当前时间；
3. 逐条处理远端商品：
   a. 如果 xianyu_item_id 已存在：
        ON CONFLICT 更新标题、价格、原价、库存、状态、分类、描述、图片、synced_at；
        不覆盖 local_product_id（保留卡密绑定）；
   b. 如果 ID 不存在，但存在“同账号 + 同标题 + 旧 ID 不在本次远端集合”的旧记录：
        认为是闲鱼重新上架导致 itemId 变化；
        把旧记录的 xianyu_item_id 改为新 ID，更新价格/库存/状态/图片等；
        同时把 products.xianyu_item_id 从旧 ID 改为新 ID；
   c. 否则插入新商品；
4. 提交事务；
5. 返回该账号当前的本地商品列表。
```

几个容易忽略的规则：

- **不是物理删除**：远端已下架/删除的商品不会被本地删除，而是标记 `off_shelf`，保留历史与卡密绑定关系。
- **只扫数字开头的 ID**：SQL 使用 `GLOB '[0-9]*'`，以数字开头的本地商品才会被批量置为 `off_shelf`；`xy_...` 这类本地临时 ID 不会误伤。
- **远端列表为空时不做下架清扫**：避免远端返回空数组（会话异常/接口异常）时把整个账号商品全部标下架。
- **保留绑定**：`ON CONFLICT` 更新不会覆盖 `local_product_id`；重新上架换 ID 时会把 `products.xianyu_item_id` 一起迁移。
- **失败回滚**：合并过程中任何 SQL 失败都会回滚事务，不会留下半套数据。

卡密绑定关系的维护：

- 「商品」页的绑定/解绑只改本地：写入 `xianyu_items.local_product_id`，同时把 `products.xianyu_item_id` 对齐到该闲鱼宝贝；
- 重新上架导致 itemId 变化时，商品同步会把旧记录的 ID、以及 `products.xianyu_item_id` 一起迁移，绑定不会丢；
- 自动发货解析商品时优先用 `xianyu_items.local_product_id`，找不到再回退 `products.xianyu_item_id`。

### 5.3 触发方式

商品同步有四条触发路径：

| 触发 | 代码位置 | 说明 |
|---|---|---|
| 页面自动同步 | `static/index.html` 15 秒定时器 | 仅当「商品」页可见、弹窗关闭、用户没在输入框内时，对当前账号调用一次同步 |
| 手动「同步闲鱼商品」 | `POST /api/xianyu/accounts/{id}/items/sync` | 立即同步当前账号 |
| 定时任务 `xianyu_item_sync` | `scheduler::sync_all_account_items` | 默认每 300 秒同步全部账号 |
| 自动发货扫描 | `run_auto_deliver_scan_cycle` | 每轮扫描前先同步该账号商品（默认 45 秒） |

> 因此默认配置下，商品实际刷新频率可能是 45 秒（自动发货扫描附带同步），而不是 300 秒。
> `xianyu_item_sync` 任务的价值在于：关闭自动发货后仍能独立同步商品，以及全账号兜底。

### 5.4 同步响应与降级

`POST /api/xianyu/accounts/{id}/items/sync` 返回：

```json
{
  "account_id": 1,
  "synced_count": 11,
  "computed_sign": "...",
  "remote_status": "SUCCESS::调用成功",
  "items": [ ... ]
}
```

- `synced_count` 是**远端返回的商品条数**，不是本地新增条数；
- 如果远端请求抛错，接口不会 500，而是返回：
  - `synced_count: 0`
  - `remote_status: 错误信息`
  - `items: 本地已有商品`
- 前端据此展示「已同步 / 登录态过期」，不会因为一次网络失败把列表清空。

### 5.5 本地 → 远端：编辑、改价、上下架

只有**真实远端数字 ID**（长度 ≥8 且全为数字）才会尝试同步到闲鱼：

| 本地动作 | 远端接口 | 失败处理 |
|---|---|---|
| 修改标题/价格/描述/重新上架 | `mtop.idle.pc.idleitem.editDetail` → `mtop.idle.pc.idleitem.edit`，必要时 `reshelf` | 本地保持原状态，不伪造修改；返回远端拒绝原因 |
| 下架 | `mtop.taobao.idle.item.downshelf/2.0/` | 同上 |
| 只改卡密绑定 | 不调用远端 | 永远只改本地 |
| 删除本地商品 | 不调用远端 | 只删本地记录，不会删除闲鱼商品 |

「杜绝假修改」是这里的关键设计：如果远端拒绝了改价/下架，本地数据库**不会**把状态改成用户想要的值，而是保留远端真实状态，并在响应里给出 `remote_attempted / remote_synced / remote_ret / remote_message`，前端会提示失败原因。

### 5.6 商品同步时序

```text
页面/定时任务/扫描
        │
        ▼
fetch_remote_items_for_account
  ├─ Cookie: 补卖家ID → 分页 item.list（最多 3×20）
  └─ OpenAPI: /api/open/product/list（50/页）
        │
        ▼
update_account_refreshed_credentials
        │
        ▼
sync_items_for_account（事务）
  ├─ 数字 ID 商品先置 off_shelf
  ├─ 已存在 → upsert（保留 local_product_id）
  ├─ 换 ID 重新上架 → 迁移旧记录与 products.xianyu_item_id
  └─ 新商品 → insert
        │
        ▼
返回本地商品列表 + remote_status
```

---

## 6. 消息同步（远端 → 本地）

### 6.1 远端拉取

入口：`platform::xianyu::scan_remote_sessions_for_account`。

流程：

1. 校验 Cookie；没有 Cookie 返回 `NO_COOKIE` 空结果；
2. 取 Cookie 中的 `cna` 作为设备 ID；
3. 调用 `mtop.taobao.idlemessage.pc.login.token/1.0/` 获取 IM 令牌；
4. 调用 `mtop.taobao.idlemessage.pc.session.sync/3.0/`：
   - 当前固定 `fetchNum = 30`；
   - 拉取最近 30 个会话；
5. 解析 `data.sessions / data.sessionList / data.list`；
6. 返回 `RemoteSessionListOutcome`。

每个会话快照 `RemoteSessionSnapshot` 的字段：

| 字段 | 来源 |
|---|---|
| `session_cid` | `cid` / `sessionId` |
| `buyer_id` | `peerUserId` / `buyerId` / `senderUserId` / `userId` |
| `buyer_nickname` | `peerNick` / `buyerNick` / `nick` / `userName` / `displayName`，并做 URL 解码 |
| `xianyu_item_id` | `itemId` / `idleItemId` |
| `order_id` | `orderId` / `tradeOrderId` / `bizOrderId`，或从文本中提取 |
| `summary` | `summary` / `lastMessage` / `content` / `text`，空则 `[无消息内容]` |
| `waiting_ship` | 摘要包含「等待你发货 / 等待卖家发货 / 我已付款 / 买家已付款 / WAIT_SELLER_SEND_GOODS」 |
| `unread_count` | `unreadCount` / `unreadNum` / `unread`（仅解析，当前落库不直接使用） |

### 6.2 本地合并

入口：`db::message::upsert_remote_sessions`。

会话唯一键：`(account_id, session_cid)`。

```text
逐条会话：
  ├─ 新会话：
  │    INSERT message_conversations（unread_count=0）
  │    插入一条 direction='in' 消息
  │    unread_count + 1
  ├─ 已存在：
  │    更新 seller_nick / buyer_id / buyer_nickname / xianyu_item_id
  │    判断 summary 是否变化：
  │      ├─ 与 last_message 相同 → 不插入、不增加未读
  │      ├─ 与最近一条入站消息相同 → 不插入（处理手动回复覆盖 last_message 的情况）
  │      └─ 不同 → 插入入站消息，unread + 1，last_message_at = 当前时间
  └─ 同一摘要永不重复插入
```

消息类型：

- `waiting_ship = true` → `msg_type = 'order_card'`
- 其他 → `msg_type = 'text'`

未读规则：

- 只有**新插入入站消息**时才 `unread_count + 1`；
- 已有未读数不会被同步覆盖为 0；
- 「标记已读」由用户打开会话时调用 `POST /api/messages/conversations/{id}/read`。

时间规则：

- `last_message_at` 只在摘要真正变化时更新；
- 避免同一会话每 60 秒同步一次就被顶到列表最前面。

计数规则：

- `synced_conversations` 统计本轮 upsert 的会话数；
- `synced_messages` 只统计本轮**新插入的入站消息**，不包含手动发送与自动回复的出站消息。

### 6.3 自动回复

前提：

- `system_settings.message_auto_reply_enabled = true`
- `message_auto_reply_template` 非空

逻辑：

1. 在 upsert **之前**判断哪些会话出现了新的非待发货文本；
2. 对比 `last_message` 和最近一条入站消息，避免把卖家自己刚发的内容误判成新消息；
3. 调用 `remote_send_im_message` 发送模板；
4. 只有远端确认发送成功，才写入一条 `direction='out'` 消息；
5. **不更新 `last_message`**，防止下一轮同步把同一摘要再次判为新消息；
6. 自动回复失败只记录到 `errors`，不影响本轮消息同步。

默认关闭自动回复。

### 6.4 手动发送、已读、删除

| 操作 | API | 行为 |
|---|---|---|
| 手动回复 | `POST /api/messages/conversations/{id}/send` | 先调闲鱼 IM；无论远端成功与否都写入出站消息；更新会话 `last_message`；返回 `remote_sent/remote_ret` |
| 标记已读 | `POST /api/messages/conversations/{id}/read` | `unread_count = 0` |
| 删除会话 | `DELETE /api/messages/conversations/{id}` | 删除本地会话及其消息，不影响闲鱼远端 |
| 查看会话 | `GET /api/messages/conversations/{id}` | 返回会话 + 按 id 正序的消息 |
| 统计 | `GET /api/messages/stats` | 会话数、未读总数、今日消息、进行中会话 |

手动发送与自动回复的差异：

- 手动发送：**先发后落库**，失败也保留出站记录，并更新 `last_message`；
- 自动回复：**只在他方确认成功后落库**，且不更新 `last_message`。

### 6.5 触发方式与轮询

| 触发 | 说明 |
|---|---|
| 定时任务 `xianyu_message_sync` | 默认每 60 秒，全账号远端同步 |
| 手动「同步消息」 | `POST /api/messages/sync`，可指定 `account_id` |
| 页面 20 秒轮询 | **只刷新本地会话列表与统计**（GET），不会重复请求闲鱼 |
| 打开会话 | 拉取该会话消息，并标记已读 |

注意：页面 20 秒轮询和后台 60 秒远端同步是两件事。前者只读本地库，后者才访问闲鱼。

### 6.6 消息同步时序

```text
定时任务/手动同步
        │
        ▼
scan_remote_sessions_for_account
  ├─ login.token
  └─ session.sync (fetchNum=30)
        │
        ▼
解析 sessions → RemoteSessionSnapshot
        │
        ▼
upsert_remote_sessions（事务）
  ├─ 会话 upsert
  ├─ 新摘要才插入入站消息
  ├─ 新入站才 unread + 1
  └─ last_message_at 仅摘要变化时更新
        │
        ├─ 自动回复（可选）
        │    └─ 远端发送成功才写入出站消息
        ▼
返回 synced_accounts / synced_conversations / synced_messages
```

### 6.7 已知边界

- 只同步**最近 30 个会话**，不是全部历史会话；
- 只拿到每个会话的**最后一条摘要**，不是完整聊天记录；
- `message_sync_limit` 设置项已存在，但平台函数当前固定 `fetchNum=30`，尚未读取该设置；
- 远端 `unreadCount` 只解析，本地未读以“新摘要插入”为准；
- 删除会话只影响本地。

---

## 7. 订单同步与自动发货

### 7.1 订单从哪里来

订单有两条发现链路，**卖家订单列表是主通道，IM 会话扫描只作为补充**：

| 来源 | 入口 | 说明 |
|---|---|---|
| 卖家订单列表同步 | `order_sync::sync_all_accounts` / `POST /api/orders/sync` | 从「待发货」订单列表导入，不依赖买家是否发过消息 |
| IM 会话扫描 | `run_auto_deliver_scan_cycle` | 从会话摘要中识别「我已付款，等待你发货」，只覆盖有聊天记录的订单 |
| Webhook | `POST /api/webhooks/xianyu/order` | 需要 `x-xianyu-token` 校验 |
| 手动发货 | `POST /api/xianyu/orders/consign` | 管理后台输入订单号执行 |
| 本地手动下单 | `POST /api/orders` | 直接对卡密商品下单，不涉及闲鱼 |

订单同步**只导入、不自动发货**：导入后的订单状态是 `pending` / `pending`，不会扣卡密，也不会调用 `consign.dummy` 或发送 IM。
这样打开订单页不会误触真实买家的发货流程；要发货时，由用户在订单行点击「重试」，或由 IM 扫描发现后走自动发货。

### 7.2 卖家订单列表同步（主通道）

入口：

- 定时任务 `xianyu_order_sync`（`kind = order_sync`，默认 60 秒）；
- 订单页加载/切换时静默同步；
- 手动按钮「同步闲鱼待发货订单」；
- API `POST /api/orders/sync`。

远端接口：

```text
mtop.taobao.idle.trade.merchant.sold.get
请求：{"pageNumber": 1, "orderStatus": "NOT_SHIP"}
响应：data.module.items[] / data.module.nextPage / data.module.totalCount
```

单条订单解析：

| 本地字段 | 远端字段 |
|---|---|
| `platform_order_id` | `commonData.orderId`（或 `orderIdStr`） |
| `xianyu_item_id` | `commonData.itemId` |
| `amount` | `priceVO.totalPrice`（元 → 分；缺失时用 `auctionPrice * buyNum`） |
| `buyer_id` | `buyerInfoVO.buyerId` |
| `buyer_nickname` | `buyerInfoVO.userNick` |
| `order_status` | `commonData.orderStatus`（只导入待发货，已发货/交易成功会跳过） |

入库流程：

1. 按 `(platform='xianyu', platform_order_id)` 查重，已存在则 `skipped_existing`；
2. 通过 `xianyu_item_id` 找本地绑定：
   - 先查 `xianyu_items.local_product_id`；
   - 再回退 `products.xianyu_item_id`；
3. 找不到绑定 → 计入 `unbound`，不伪造 `product_id`，并在 `errors` 中给出前 5 条提示；
4. 找到绑定 → `import_pending_remote_order` 插入 `orders(order_status='pending', delivery_status='pending')`，**不抽卡密、不调用远端**；
5. 单账号失败不影响其他账号，错误汇总到 `errors`；
6. 握手刷新出的 Cookie 只在“远端调用成功 + 确实变化”时回写，且必须仍含精确名为 `_m_h5_tk` 的字段（只有 `_m_h5_tk_enc` 视为降级），避免把一次失败握手落库成永久失效。

同步返回示例：

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

> 这条链路解决了“买家直接拍下、从未发过 IM 消息”的订单盲区。此前只靠 `session.sync`，这类订单既没有会话，也没有 `orderId`，因此永远无法被发现。

### 7.3 IM 会话扫描（补充通道）

入口：`handlers::xianyu::run_auto_deliver_scan_cycle`。

每个账号：

1. **先同步商品**（`fetch_remote_items_for_account` + `sync_items_for_account`）；
   - 失败只忽略，不阻断后续消息扫描；
2. 调用 `scan_remote_paid_orders_for_account`：
   - IM token；
   - `session.sync`；
   - 解析包含「等待你发货 / 等待卖家发货 / 我已付款 / 买家已付款 / WAIT_SELLER_SEND_GOODS」的会话；
   - 提取 `order_id`、`xianyu_item_id`、`buyer_id`、`buyer_nickname`、`session_cid`；
3. 对每个候选订单：
   - 如果 `auto_delivery_dry_run = true`：只统计，不创建订单、不扣卡密、不调用 consign/IM 写接口；
   - 否则调用 `fulfill_xianyu_order_with_remote_consign(..., source="auto_scan")`。

注意：

- `session.sync` 的真实响应只有 `message.summary.summary` 和 `session.*`，**通常没有 `orderId`**；只有摘要恰好是待发货卡片且能提取到订单号时才会命中，因此它只能作为补充；
- 演练模式只拦截“订单履约”，每轮扫描开始时的**商品同步仍会执行**；
- `/api/auto-delivery/scan` 返回的 `delivery_summary.failed` 当前固定为 0，失败详情以「发货记录」为准。

扫描返回：

```json
{
  "scanned_accounts": 1,
  "total_sessions_scanned": 30,
  "detected_orders_count": 0,
  "fulfilled_orders": [],
  "accounts": [ ... ],
  "message": "已扫描 1 个闲鱼账号...识别待发货订单 0 笔，自动发货完成 0 笔"
}
```

### 7.4 单笔发货的完整步骤

入口：`fulfill_xianyu_order_with_remote_consign`。

```text
1. 规范化 platform_order_id，空则报错
2. 解析要发的卡密商品：
   ├─ 有 xianyu_item_id：
   │    ├─ 查 xianyu_items.local_product_id
   │    └─ 回退 products.xianyu_item_id
   ├─ 否则用请求里的 product_id
   └─ 都找不到 → 报错“尚未绑定卡密商品”
3. create_order_with_input（事务 + 幂等）：
   ├─ 若 (platform='xianyu', platform_order_id) 已存在且已发货 → 直接返回历史订单
   ├─ 若已存在但 order_status='pending' / delivery_status='pending' 且没有卡密
   │    → 继续抽卡密，并把原订单行升级为 delivered/success（复用订单 ID）
   │    → 升级时**不覆盖 amount**（保留买家真实实付金额），
   │      买家/账号/宝贝字段用 COALESCE 兜底，避免调用方没带就写成 NULL
   ├─ 校验商品 enabled
   ├─ 按模板类型分配发货内容：
   │    ├─ redeem_code：取最早 unused 卡密，标记 sold
   │    ├─ file_link：取模板固定内容
   │    └─ steam_gift：生成 AppID 任务文本
   ├─ 新订单插入 orders（order_status='delivered', delivery_status='success'）
   └─ INSERT 撞唯一索引（与订单同步并发）→ 回滚后按已存在订单处理；
      若赢家仍是未发货的 pending 单，则重跑一次升级路径真正发货，而不是静默返回重复
4. 读取发货模板（system_settings 优先，回退 config.toml）
5. 读取全局开关 auto_consign / auto_send_message
6. 幂等保护：
   └─ 若 is_duplicate 且 source 不是 retry/manual → 直接返回，不再调远端
7. 自动确认发货（可选）：
   └─ remote_consign_dummy_order
8. 自动发送卡密（可选）：
   └─ remote_send_im_message
9. 包装函数写 delivery_records：
   ├─ 成功 → status='success'，记录远端 consign/IM 返回
   ├─ 失败 → status='failed'，记录 error_message
   └─ 巡检重复发现的成功订单不重复写记录
10. 按 system_settings 发通知：
   ├─ auto_delivery_notify_on_success
   └─ auto_delivery_notify_on_failure
```

### 7.5 幂等与重复发货保护

三层幂等：

1. **订单层**：`orders(platform, platform_order_id)` 唯一索引；
2. **卡密层**：`UPDATE redeem_codes SET status='sold' WHERE id=? AND status='unused'`，并发只有一条能成功；
3. **远端调用层**：`is_duplicate && source ∉ {retry, manual}` 时直接返回，不再调用 `consign.dummy` / IM，避免给买家重复发卡密。

不同来源的语义：

| source | 重复订单时是否重发远端 |
|---|---|
| `auto_scan` | 否 |
| `webhook` | 否 |
| `retry` | 是 |
| `manual` | 是 |
| `api` | 当前未使用 |

### 7.6 自动发货配置

页面：「自动发货」

| 设置 | 作用 |
|---|---|
| `auto_delivery_enabled` | 关闭后调度器跳过自动扫描（手动扫描接口仍可调用） |
| `auto_delivery_scan_interval` | 扫描间隔；保存时同步到 `xianyu_auto_deliver` 任务 |
| `auto_delivery_auto_consign` | 是否调用闲鱼虚拟发货 |
| `auto_delivery_auto_send_message` | 是否通过 IM 发送卡密 |
| `auto_delivery_reply_template` | 发货消息模板，支持 `{product_name}` `{order_id}` `{delivery_content}` |
| `auto_delivery_dry_run` | 演练模式：只识别待发货，不建单、不扣卡密、不调用 consign/IM 写接口（商品同步与只读扫描仍会执行） |
| `auto_delivery_retry_limit` | 失败重试上限（当前重试任务固定每轮最多 5 条，未读取此值） |
| `auto_delivery_low_stock_threshold` | 低库存预警阈值 |
| `auto_delivery_notify_on_success` | 发货成功是否发通知 |
| `auto_delivery_notify_on_failure` | 发货失败是否发通知 |

配置读取优先级：

```text
system_settings.auto_delivery_reply_template
        ↓ 为空时回退
config.toml [xianyu].reply_template
```

### 7.7 订单状态与发货状态

`orders` 表有两组状态：

- `order_status`：`pending | paid | delivering | delivered | failed | refunded | closed`
- `delivery_status`：`pending | success | failed`

自动发货的顺序是**先发卡密、后确认发货**：

1. `create_order_with_input` 扣卡密并把订单写成 `delivered/success`（这是本地“已分配发货内容”的记账）；
2. 通过闲鱼私信网关把卡密发给买家；
3. **只有消息被服务端确认（`message_sent = true`），才会调用 `consign.dummy` 确认发货**；
4. 消息发送失败 → 订单改回 `failed/failed`，闲鱼侧保持「待发货」，不会出现“订单已收货但买家什么都没收到”。

远端结果的排查位置：

- `delivery_records.status`：只有 IM 与 consign 都成功才是 `success`，否则为 `failed`
- `delivery_records.remote_consign_status`
- `delivery_records.remote_im_status`
- `delivery_records.error_message`
- 操作日志/通知日志

#### 私信通道（WebSocket）

闲鱼私信没有可用的 MTOP HTTP 接口（`mtop.taobao.idlemessage.pc.message.send` 稳定返回 `FAIL_SYS_API_NOT_FOUNDED`），
真实通道是钉钉 IM 网关 `wss://wss-goofish.dingtalk.com/`，实现见 `src/platform/xianyu_im.rs`：

```text
1. mtop.taobao.idlemessage.pc.login.token  → accessToken（并统一生成 did）
2. wss 连接（Cookie + Origin: https://www.goofish.com）
3. /reg 注册，随后 /r/SyncStatus/ackDiff
4. 没有会话时 /r/SingleChatConversation/create → body.singleChatConversation.cid
5. /r/MessageSend/sendByReceiverScope，content.custom.data = base64({"contentType":1,"text":{"text":"..."}})
6. 等待 headers.mid 匹配的响应，code == 200 且 body.messageId 非空才算发送成功
```

注意：卖家**不能**给任意用户凭空建会话（返回 `ILLEGAL_SESSION_CREATE_BY_SELLER`），
只能给与自己有订单关系的买家创建；买家从未聊天时依赖这一步建立会话。

### 7.8 订单与发货的已知边界

- 订单列表同步只拉 `orderStatus=NOT_SHIP` 的待发货订单，不是全量历史；已发货、已退款、已关闭的订单不会回写本地状态；
- 订单同步只入库，不自动发货；导入后的 `pending` 订单需要用户点击订单行「发货」或由 IM 扫描触发履约；
- 找不到本地绑定商品的订单会计入 `unbound` 并跳过，不会伪造 `product_id`；绑定后再次同步即可导入；
- 远端 consign 或 IM 失败会把本地订单改成 `failed/failed`，发货记录同步写 `failed`；
- 巡检重复发现**成功订单**不会重复发货；但同一订单如果持续失败，可能按扫描周期产生多条失败发货记录；
- `delivery_retry` 任务默认关闭；开启后只重试最近 7 天、带 `platform_order_id` 的失败记录，每轮最多 5 条（当前未读取 `auto_delivery_retry_limit`）。

---

## 8. 发货记录、通知与日志

### 8.1 发货记录

`delivery_records` 每次发货尝试写一行：

| 字段 | 说明 |
|---|---|
| `order_id` | 本地订单 ID（失败在创建订单前则为空） |
| `account_id` / `xianyu_item_id` / `platform_order_id` | 来源定位 |
| `product_id` / `product_name` | 卡密商品 |
| `buyer_nickname` | 买家 |
| `delivery_content` | 实际发货内容（成功时） |
| `status` | `success / failed / pending` |
| `source` | `auto_scan / webhook / manual / api / retry` |
| `remote_consign_status` | 远端虚拟发货返回 |
| `remote_im_status` | 远端 IM 返回 |
| `error_message` | 失败原因 |
| `duration_ms` | 本次尝试耗时 |

页面：「发货记录」支持筛选、详情、重试、删除、导出 CSV。

### 8.2 通知

事件规则：

```text
事件发生
  → 查 notification_rules（enabled=1）
  → join notification_channels（enabled=1）
  → 渲染模板 {event}/{title}/{content}/{time}
  → 按渠道发送
  → 写 notification_logs（sent/failed）
```

当前代码**自动触发**的事件只有：

- `delivery_success`
- `delivery_failed`

通知设置页还预留了 `account_offline`、`out_of_stock`、`task_failed`、`order_refunded`、`system_alert`，这些事件类型可以建规则，但当前没有代码自动派发，属于后续扩展点。

渠道：

- Webhook（可带 `X-Webhook-Secret`）
- 飞书群机器人
- ServerChan
- 邮件/短信（需要配置 `gateway_url`，否则明确发送失败）

### 8.3 三类日志的区别

| 日志 | 表 | 记录内容 | 入口 |
|---|---|---|---|
| 操作日志 | `operation_logs` | 谁在什么时间调用了哪个变更 API、状态码、IP、UA、耗时 | 「操作日志」 |
| 任务执行日志 | `scheduled_task_runs` | 定时任务每次执行的成功/失败、消息、耗时 | 「定时任务」 |
| 连接日志 | `xianyu_connection_logs` | 登录、验签、重连、健康检查 | 「闲鱼账号 → 连接日志」 |

操作日志由中间件自动写入：

- 范围：`POST/PUT/PATCH/DELETE /api/*`
- 排除：`/api/webhooks/*`
- 不读取请求体，避免记录密码、卡密等敏感内容
- `detail` 字段当前通常为空，排障主要看 `action/path/status_code/actor/ip`

---

## 9. 数据清理与保留

`system_cleanup` 默认每天执行一次，按设置删除：

| 数据 | 设置键 | 默认 |
|---|---|---|
| 操作日志 | `log_retention_days` | 30 天 |
| 发货记录 | `delivery_retention_days` | 90 天 |
| 通知日志 | `notification_retention_days` | 30 天 |
| 任务执行记录 | 固定 | 7 天 |

不清理：

- `orders`
- `message_conversations` / `messages`
- `xianyu_items`
- `xianyu_connection_logs`

这些需要手工维护或后续扩展。

---

## 10. 配置项速查

| 键 | 默认值 | 影响 |
|---|---|---|
| `auto_delivery_enabled` | `true` | 调度器是否执行自动扫描 |
| `auto_delivery_scan_interval` | `45` | 扫描间隔，同时写入任务表 |
| `auto_delivery_auto_consign` | `true` | 是否调 `consign.dummy` |
| `auto_delivery_auto_send_message` | `true` | 是否 IM 发卡密 |
| `auto_delivery_reply_template` | 内置模板 | 发货消息模板 |
| `auto_delivery_dry_run` | `false` | 演练模式 |
| `auto_delivery_retry_limit` | `3` | 失败重试上限（当前重试任务固定每轮最多 5 条，未读取此值） |
| `auto_delivery_low_stock_threshold` | `5` | 低库存阈值 |
| `auto_delivery_notify_on_success` | `false` | 成功通知开关 |
| `auto_delivery_notify_on_failure` | `true` | 失败通知开关 |
| `message_sync_enabled` | `true` | 调度器是否执行消息同步 |
| `message_auto_reply_enabled` | `false` | 是否自动回复 |
| `message_auto_reply_template` | 内置文案 | 自动回复模板 |
| `message_sync_limit` | `30` | 预留，当前平台层固定 30 |
| `log_retention_days` | `30` | 操作日志保留 |
| `delivery_retention_days` | `90` | 发货记录保留 |
| `notification_retention_days` | `30` | 通知日志保留 |
| `session_timeout_minutes` | `720` | 管理员会话有效期 |
| `password_min_length` | `8` | 管理员密码最小长度 |

---

## 11. 排障指南

| 现象 | 优先检查 | 常见原因 |
|---|---|---|
| 商品列表不更新 | 「闲鱼账号」连接日志、同步返回的 `remote_status` | Cookie 过期、`_m_h5_tk` 失效、OpenAPI 凭证错误 |
| 远端已下架但本地还在售 | 是否执行过商品同步；商品 ID 是否为真实远端数字 ID | 未同步、非远端 ID、远端接口拒绝 |
| 本地改价/下架没生效 | 响应里的 `remote_attempted/remote_synced/remote_ret` | 远端拒绝；系统不会伪造成功 |
| 消息不同步 | `xianyu_message_sync` 任务状态、`session_sync_ret` | IM token 失败、Cookie 过期、最近 30 个会话之外 |
| 聊天记录只有一条 | 设计边界 | `session.sync` 只返回每个会话最后摘要 |
| 不自动发货 | 「自动发货 → 就绪检查」 | 商品禁用、未绑定、卡密不足、商品下架、账号异常 |
| 扫描到订单但没发货 | `auto_delivery_enabled`、`dry_run`、`fulfill` 错误、发货记录 | 开关关闭、演练模式、未绑定卡密、扣码失败 |
| 买家收到重复卡密 | `delivery_records` 是否重复、订单号是否变化 | 订单号不一致会被当成新订单；检查 `platform_order_id` |
| 订单列表没有闲鱼订单 | `xianyu_order_sync` 任务状态、`POST /api/orders/sync` 返回的 `errors/unbound` | 订单同步未执行、Cookie 过期、商品未绑定、远端接口异常 |
| 通知没发 | 渠道 enabled、规则 enabled、通知日志 | 渠道关闭、规则未匹配、网关未配置 |
| 操作日志没有某次操作 | 请求方法是否为 GET；是否 Webhook | 中间件只记录变更类 `/api/*` |
| 定时任务不执行 | 任务 enabled、`next_run_at`、`last_status`、执行历史 | 被禁用、正在运行、间隔未到、上次失败 |

常用排查命令：

```bash
# 查看任务状态
curl -s http://127.0.0.1:8080/api/scheduler/tasks \
  -H "Authorization: Bearer <token>" | python3 -m json.tool

# 手动触发商品同步
curl -s -X POST http://127.0.0.1:8080/api/xianyu/accounts/1/items/sync \
  -H "Authorization: Bearer <token>" | python3 -m json.tool

# 手动触发消息同步
curl -s -X POST http://127.0.0.1:8080/api/messages/sync \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/json' -d '{}' | python3 -m json.tool

# 手动同步闲鱼待发货订单（只入库，不自动发货）
curl -s -X POST http://127.0.0.1:8080/api/orders/sync \
  -H "Authorization: Bearer <token>" | python3 -m json.tool

# 手动扫描 IM 待发货卡片并自动发货
curl -s -X POST http://127.0.0.1:8080/api/auto-delivery/scan \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/json' -d '{}' | python3 -m json.tool

# 查看连接日志
curl -s "http://127.0.0.1:8080/api/xianyu/accounts/1/connection-logs?limit=20" \
  -H "Authorization: Bearer <token>" | python3 -m json.tool
```

---

## 12. 设计边界与后续改进

当前实现刻意保持“单进程 + SQLite + 轮询”的简单结构，已知边界：

1. **消息不是 WebSocket 实时推送**，而是 60 秒轮询 `session.sync`；
2. **聊天记录不是完整历史**，只有每个会话最后一条摘要；
3. **消息同步上限固定 30**，`message_sync_limit` 尚未接入平台函数；
4. **订单只同步待发货状态**，已发货/退款/关闭状态不会回写；且订单同步只导入，不自动履约；
5. **远端 consign/IM 失败不会回写订单状态**，只记录在发货记录的远端字段；
6. **失败发货可能按扫描周期重复记录**，目前没有“同一订单失败去重窗口”；
7. **自动发货和消息中心各自调用一次 `session.sync`**，同一分钟内可能请求两次；
8. **调度器没有跨进程分布式锁**，只适合单实例部署。

如果后续要继续增强，优先级建议：

1. 把 `session.sync` 抽成一次调用，同时喂给消息中心和自动发货；
2. 接入完整消息历史接口，支持分页聊天记录；
3. 给失败发货记录加“同订单冷却窗口”，避免重复告警；
4. 把远端 consign/IM 结果回写 `orders.delivery_status`；
5. 用 WebSocket 替换轮询，降低延迟与风控压力；
6. 多实例部署时给 `scheduled_tasks` 增加租约/分布式锁。
