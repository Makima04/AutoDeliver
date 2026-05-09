# 自动发货平台 — 项目规划文档

## 一、项目概述

### 1.1 项目名称
**Auto-Delivery Platform** — 虚拟商品自动发货平台

### 1.2 项目目标
为淘宝、闲鱼等平台的虚拟商品卖家提供一套自动化发货系统。卖家在平台上架商品后，系统自动监听订单，根据商品类型执行对应的发货逻辑（生成兑换码、Steam 机器人赠送礼物等），实现 7x24 小时无人值守发货。

### 1.3 核心场景

**场景 A：AI Token 兑换码**
- 卖家在后台批量生成兑换码并导入系统
- 买家在淘宝下单后，系统自动从库存中取出一个未使用的兑换码
- 通过淘宝消息/订单备注将兑换码发送给买家

**场景 B：Steam 游戏礼物**
- 卖家在后台配置 Steam 机器人账号和游戏库存
- 买家下单时提供 Steam 好友链接或 ID
- 系统操控 Steam 机器人自动添加好友、发送礼物

### 1.4 用户画像
- 个人卖家或小团队，同时在多个平台开店
- 日均订单量 10-200 单
- 不想手动复制粘贴发货，希望自动化

---

## 二、系统架构

### 2.1 整体架构图（文字版）

```
┌─────────────────────────────────────────────────────────┐
│                    卖家浏览器（管理后台）                   │
│              http://localhost:8080/dashboard              │
└──────────────────────┬──────────────────────────────────┘
                       │ HTTP / WebSocket
                       ▼
┌─────────────────────────────────────────────────────────┐
│                   auto-delivery 服务                      │
│  ┌────────────────────────────────────────────────────┐  │
│  │                  axum Web 框架                       │  │
│  │  /api/products  /api/orders  /api/codes  /ws        │  │
│  └────────────────────────────────────────────────────┘  │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌─────────┐ │
│  │ 平台适配层 │  │ 发货引擎  │  │ 库存管理  │  │ 定时任务 │ │
│  │ (trait)   │  │ (核心)    │  │ (兑换码)  │  │ (调度)  │ │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬────┘ │
│       │             │             │              │       │
│  ┌────┴─────────────┴─────────────┴──────────────┴───┐  │
│  │              SQLite / PostgreSQL 数据库              │  │
│  └───────────────────────────────────────────────────┘  │
└────────┬─────────────────────┬──────────────────────────┘
         │                     │
         ▼                     ▼
  ┌──────────────┐     ┌──────────────┐
  │  淘宝开放平台  │     │  Steam API   │
  │  (订单轮询)   │     │  (机器人控制) │
  └──────────────┘     └──────────────┘
```

### 2.2 核心模块说明

| 模块 | 职责 | 关键 Rust 知识点 |
|------|------|-----------------|
| **config** | 读取配置文件，管理数据库连接、API 密钥等 | serde 反序列化、toml |
| **db** | 数据库操作层，所有 SQL 集中管理 | sqlx、async/await |
| **models** | 数据结构定义（Product、Order、RedeemCode 等） | struct、enum、impl |
| **platform** | 平台适配层，统一不同平台的接口差异 | trait、泛型、动态分发 |
| **delivery** | 发货引擎核心，接收订单后执行发货 | trait 对象、错误处理 |
| **scheduler** | 定时任务：轮询订单、检查超时、清理过期码 | tokio spawn、定时器 |
| **api** | HTTP 接口层，给管理后台提供 REST API | axum、JSON 序列化 |
| **steam-bot** | Steam 机器人控制：登录、加好友、送礼物 | FFI、外部 crate |

---

## 三、技术选型

### 3.1 核心技术栈

| 层次 | 选择 | 理由 |
|------|------|------|
| **语言** | Rust (2024 Edition) | 你正在学习，且性能优秀、类型安全 |
| **Web 框架** | axum | Tokio 官方出品，类型安全的路由和提取器，社区活跃，文档完善 |
| **异步运行时** | tokio | Rust 异步生态的事实标准 |
| **数据库** | SQLite (开发) / PostgreSQL (生产) | 开发阶段零配置，生产可迁移 |
| **ORM/查询** | sqlx | 异步、编译期校验 SQL 语法和类型 |
| **序列化** | serde + serde_json | Rust 生态标准，处理 JSON 数据 |
| **HTTP 客户端** | reqwest | 调用淘宝等外部 API |
| **日志** | tracing + tracing-subscriber | Tokio 生态标准日志方案 |
| **配置管理** | toml + serde | 读取 config.toml |
| **错误处理** | anyhow (应用层) + thiserror (库层) | 简洁的错误传播 |
| **模板引擎** | askama | 编译期模板渲染，类型安全 |
| **Steam 集成** | steamworks-rs 或 steam-rs | Steam 官方 SDK 绑定 |

### 3.2 选型对比：axum vs actix-web

| 维度 | axum | actix-web |
|------|------|-----------|
| 出品方 | Tokio 官方 | 社区 |
| 学习曲线 | 平缓 | 稍陡 |
| 类型安全 | 高（提取器系统） | 高 |
| 异步生态 | 与 tokio 深度整合 | 需要 actix-rt |
| 中间件 | Tower 中间件 | 自有中间件 |
| 社区活跃度 | 高，增长快 | 成熟稳定 |

**结论：选择 axum**。对 Rust 初学者更友好，且与 tokio 生态无缝衔接。

### 3.3 选型对比：sqlx vs diesel

| 维度 | sqlx | diesel |
|------|------|--------|
| 异步支持 | 原生异步 | 需要 diesel-async |
| SQL 方式 | 写原生 SQL | DSL 查询构建器 |
| 编译期校验 | 有（sqlx::query! 宏） | 有（更严格） |
| 学习成本 | 低（会写 SQL 就行） | 中（需要学 DSL） |
| 数据库支持 | SQLite, PostgreSQL, MySQL | 类似 |

**结论：选择 sqlx**。你已经会写 SQL，学习成本低，且异步支持原生。

---

## 四、数据库设计

### 4.1 核心表

**products（商品表）**

| 字段 | 类型 | 说明 |
|------|------|------|
| id | INTEGER PK | 自增主键 |
| name | TEXT | 商品名称 |
| product_type | TEXT | 类型：redeem_code / steam_gift |
| platform | TEXT | 来源平台：taobao / xianyu |
| platform_item_id | TEXT | 平台上的商品 ID |
| description | TEXT | 商品描述 |
| price | INTEGER | 价格（单位：分） |
| stock_count | INTEGER | 库存数量 |
| enabled | BOOLEAN | 是否启用 |
| created_at | DATETIME | 创建时间 |

**redeem_codes（兑换码表）**

| 字段 | 类型 | 说明 |
|------|------|------|
| id | INTEGER PK | 自增主键 |
| product_id | INTEGER FK | 关联商品 |
| code | TEXT UNIQUE | 兑换码内容 |
| status | TEXT | unused / sold / expired |
| sold_order_id | INTEGER FK | 售出关联的订单 |
| created_at | DATETIME | 创建时间 |
| used_at | DATETIME | 使用时间 |

**orders（订单表）**

| 字段 | 类型 | 说明 |
|------|------|------|
| id | INTEGER PK | 自增主键 |
| platform | TEXT | 平台：taobao / xianyu / manual |
| platform_order_id | TEXT UNIQUE | 平台订单号 |
| product_id | INTEGER FK | 关联商品 |
| buyer_id | TEXT | 买家 ID |
| buyer_nickname | TEXT | 买家昵称 |
| amount | INTEGER | 订单金额（分） |
| status | TEXT | pending / delivering / delivered / failed |
| delivery_content | TEXT | 实际发货内容（兑换码等） |
| error_message | TEXT | 错误信息 |
| created_at | DATETIME | 创建时间 |
| delivered_at | DATETIME | 发货时间 |

**steam_accounts（Steam 机器人表）**

| 字段 | 类型 | 说明 |
|------|------|------|
| id | INTEGER PK | 自增主键 |
| username | TEXT | Steam 用户名 |
| password_encrypted | TEXT | 加密后的密码 |
| shared_secret | TEXT | Steam Guard 密钥 |
| status | TEXT | idle / busy / offline |
| last_login_at | DATETIME | 最后登录时间 |

**steam_inventory（Steam 游戏库存表）**

| 字段 | 类型 | 说明 |
|------|------|------|
| id | INTEGER PK | 自增主键 |
| bot_account_id | INTEGER FK | 关联机器人账号 |
| product_id | INTEGER FK | 关联商品 |
| game_name | TEXT | 游戏名称 |
| count | INTEGER | 库存数量 |

### 4.2 ER 关系

```
products  1──N  redeem_codes     (一个商品对应多个兑换码)
products  1──N  orders           (一个商品对应多个订单)
products  1──N  steam_inventory  (一个商品对应多个 Steam 库存)
orders    N──1  redeem_codes     (一个订单使用一个兑换码)
steam_accounts 1──N steam_inventory (一个机器人管理多个库存)
```

---

## 五、开发阶段与 Rust 学习路径

本项目分为 **6 个阶段**，每个阶段结合 Rust 知识点递进学习。

### 阶段 1：项目骨架与基础配置（第 1-2 周）

**目标：** 搭建项目结构，实现配置加载和数据库初始化。

**Rust 知识点：**
- Cargo 项目结构、模块系统（mod、pub、use）
- 结构体（struct）定义与 impl 块
- 枚举（enum）进阶用法
- serde 反序列化
- sqlx 数据库连接与建表

**具体任务：**
1. 创建 Cargo 项目，规划目录结构
2. 实现 config.toml 读取
3. 连接 SQLite 数据库
4. 编写建表 SQL（migrations）
5. 定义核心数据模型（struct）

**里程碑：** 能运行程序，读取配置，连接数据库，打印 "服务启动成功"。

---

### 阶段 2：兑换码管理与核心逻辑（第 3-4 周）

**目标：** 实现兑换码的生成、导入、查询、标记已使用。

**Rust 知识点：**
- trait 定义与实现（数据库操作抽象）
- 错误处理深入（Result、?、自定义错误）
- 迭代器（map、filter、collect）
- 随机数生成（rand crate）
- 单元测试（#[test]、#[cfg(test)]）

**具体任务：**
1. 实现兑换码生成算法（随机字符串）
2. 实现兑换码批量导入（CSV 解析）
3. 实现库存查询（可用数量统计）
4. 实现"分配一个兑换码"逻辑（原子操作）
5. 编写单元测试

**里程碑：** 能通过代码生成 100 个兑换码，查询库存，分配兑换码并标记已使用。

---

### 阶段 3：Web API 与管理后台（第 5-6 周）

**目标：** 提供 REST API 和 Web 管理界面。

**Rust 知识点：**
- axum 路由与处理器（Handler）
- axum 提取器（Path、Query、Json、State）
- JSON 序列化/反序列化
- 模板渲染（askama）
- 错误处理与 HTTP 响应

**具体任务：**
1. 搭建 axum Web 服务
2. 实现商品 CRUD API
3. 实现兑换码管理 API（导入、查询、删除）
4. 实现订单管理 API（列表、详情、状态更新）
5. 渲染管理后台页面（商品列表、兑换码列表、订单列表）
6. 实现仪表盘（今日订单数、销售额、库存告警）

**里程碑：** 浏览器打开后台，能看到商品列表、兑换码库存、订单状态。

---

### 阶段 4：发货引擎与订单处理（第 7-8 周）

**目标：** 实现核心发货逻辑，订单来了能自动分配兑换码并发货。

**Rust 知识点：**
- trait 对象（dyn Trait）—— 统一不同发货方式
- 闭包与回调
- tokio 异步编程深入（spawn、select!）
- 状态机设计模式
- 事务处理（数据库事务）

**具体任务：**
1. 定义发货接口 trait DeliveryHandler
2. 实现 RedeemCodeHandler（兑换码发货）
3. 实现订单处理流水线：新订单 → 匹配商品 → 调用对应 Handler → 更新状态
4. 实现手动下单测试接口
5. 实现发货失败重试机制
6. 添加订单状态变更日志

**里程碑：** 通过 API 创建一个订单，系统自动分配兑换码，订单状态变为"已发货"。

---

### 阶段 5：平台对接 — 淘宝（第 9-11 周）

**目标：** 对接淘宝开放平台，实现真实订单的自动监听。

**Rust 知识点：**
- reqwest HTTP 客户端
- OAuth 2.0 流程实现
- HMAC 签名（淘宝 API 签名算法）
- 定时任务（tokio 定时器）
- 异步流处理

**具体任务：**
1. 注册淘宝开放平台应用，获取 App Key/Secret
2. 实现 OAuth 授权流程（卖家登录 → 获取 Token）
3. 实现淘宝 API 签名（HMAC-MD5）
4. 实现订单增量查询（轮询 or Webhook）
5. 将淘宝订单映射为本地订单
6. 实现发货结果回传（更新淘宝订单状态）
7. 处理 Token 刷新、接口限流

**里程碑：** 淘宝下单后，系统自动检测到新订单，自动发货，淘宝订单状态更新为已发货。

---

### 阶段 6：Steam 机器人（第 12-15 周）

**目标：** 实现 Steam 机器人自动加好友、赠送游戏。

**Rust 知识点：**
- FFI（调用 C 库）—— steamworks-rs 原理
- 异步事件驱动编程
- 多线程与并发（Arc、Mutex、RwLock）
- 外部 crate 集成

**具体任务：**
1. 集成 steamworks-rs crate
2. 实现 Steam 账号登录（处理 Steam Guard）
3. 实现好友请求自动接受
4. 实现交易报价（Trade Offer）创建与发送
5. 实现 SteamGiftHandler —— 对接发货引擎
6. 实现多机器人轮换调度
7. 处理机器人掉线重连

**里程碑：** Steam 订单下单后，机器人自动添加买家好友，赠送游戏。

---

## 六、项目目录结构

```
auto-delivery/
├── Cargo.toml                  # 项目配置与依赖
├── config.toml                 # 运行配置（不提交 git）
├── config.example.toml         # 配置示例
├── migrations/                 # 数据库迁移脚本
│   ├── 001_create_products.sql
│   ├── 002_create_redeem_codes.sql
│   ├── 003_create_orders.sql
│   └── 004_create_steam_accounts.sql
├── src/
│   ├── main.rs                 # 入口：初始化配置、数据库、启动服务
│   ├── config.rs               # 配置文件读取
│   ├── error.rs                # 统一错误类型
│   ├── db/                     # 数据库操作层
│   │   ├── mod.rs
│   │   ├── products.rs
│   │   ├── redeem_codes.rs
│   │   ├── orders.rs
│   │   └── steam_accounts.rs
│   ├── models/                 # 数据模型
│   │   ├── mod.rs
│   │   ├── product.rs
│   │   ├── redeem_code.rs
│   │   ├── order.rs
│   │   └── steam_account.rs
│   ├── handlers/               # axum 路由处理器
│   │   ├── mod.rs
│   │   ├── products.rs
│   │   ├── redeem_codes.rs
│   │   ├── orders.rs
│   │   └── dashboard.rs
│   ├── delivery/               # 发货引擎
│   │   ├── mod.rs
│   │   ├── traits.rs           # 发货接口定义
│   │   ├── redeem_code.rs      # 兑换码发货实现
│   │   └── steam_gift.rs       # Steam 礼物发货实现
│   ├── platform/               # 平台对接
│   │   ├── mod.rs
│   │   ├── taobao/
│   │   │   ├── mod.rs
│   │   │   ├── auth.rs
│   │   │   ├── client.rs
│   │   │   └── orders.rs
│   │   └── steam/
│   │       ├── mod.rs
│   │       ├── bot.rs
│   │       └── trade.rs
│   ├── scheduler/              # 定时任务
│   │   ├── mod.rs
│   │   └── tasks.rs
│   └── templates/              # askama HTML 模板
│       ├── layout.html
│       ├── dashboard.html
│       ├── products.html
│       ├── codes.html
│       └── orders.html
├── static/                     # 静态资源
│   ├── css/
│   └── js/
├── tests/                      # 集成测试
│   ├── api_tests.rs
│   └── delivery_tests.rs
├── docs/                       # 文档
│   └── auto-delivery-plan.md   # 本文档
└── Dockerfile
```

---

## 七、里程碑时间线

```
Week 1-2    ██████░░░░░░░░░░░░░░  阶段1：项目骨架
Week 3-4    ██████████░░░░░░░░░░  阶段2：兑换码管理
Week 5-6    ██████████████░░░░░░  阶段3：Web 管理后台
Week 7-8    ████████████████░░░░  阶段4：发货引擎
Week 9-11   ██████████████████░░  阶段5：淘宝对接
Week 12-15  ████████████████████  阶段6：Steam 机器人
```

**总预估周期：** 12-15 周（边学边做）

---

## 八、风险与注意事项

### 8.1 淘宝开放平台
- 淘宝 API 申请需要实名认证和开发者资质
- API 调用有频率限制，需要做好限流
- 淘宝政策可能变化，需要关注官方公告
- 闲鱼目前没有官方开放 API，可能需要半自动方案

### 8.2 Steam
- Steam 机器人账号可能被 VAC 封禁，需要谨慎操作
- steamworks-rs 生态不如 Node.js 的 steam-user 成熟
- Steam Guard 两步验证增加自动化复杂度
- 建议先用测试账号验证

### 8.3 安全
- 兑换码需要加密存储，防止数据库泄露
- Steam 密码和 Token 需要加密
- 管理后台需要认证机制（至少密码保护）
- API 接口需要防重放攻击

### 8.4 Rust 学习曲线
- 前期会遇到很多编译错误，这是正常的
- 所有权和借用可能让你困惑，但结合项目实践会理解更快
- 异步编程（async/await）需要一定时间适应
- 遇到困难时可以先用简单方案实现，后期重构优化

---

## 九、下一步

确认好阶段划分和整体方向后，我们可以从 **阶段 1** 开始，逐个文件地实现，边写边讲解 Rust 知识点。
