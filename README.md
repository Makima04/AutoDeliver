# AutoDeliver — 闲鱼多账号自动发货平台

基于 Rust + axum + SQLite 的闲鱼虚拟商品自动发货系统。参考 [xianyu-pilot-pro](https://github.com/dameng2026/xianyu-pilot-pro) 的产品能力，用 Rust 单进程实现了管理后台、闲鱼账号接入、商品/订单、在线消息、自动发货、发货审计、定时任务、通知与系统管理。

> ⚠️ 本项目仅供学习研究。请遵守闲鱼平台服务协议与当地法律法规，勿用于违规批量操作。

---

## 功能模块

| 模块 | 能力 |
|------|------|
| **闲鱼账号登录与连接管理** | 手机扫码登录、Cookie 登录、闲鱼管家 OpenAPI；多账号、验签、重连、连接健康度与连接日志 |
| **商品管理** | 闲鱼商品同步/新增/编辑/改价/上下架、绑定卡密商品、状态筛选、库存统计 |
| **卡密库存中心** | 卡密商品 CRUD、批量导入兑换码、单条删除、库存统计与低库存预警 |
| **订单管理** | 待发货订单列表同步（不依赖 IM）、订单列表/统计、状态流转、闲鱼远端重试发货、IM 补发、删除与订单详情 |
| **在线消息** | 闲鱼 IM 会话同步、会话列表/未读、聊天记录、直接回复买家、可配置自动回复 |
| **自动发货** | 全局开关、扫描间隔、自动虚拟发货、自动 IM 发卡密、消息模板、演练模式、发货就绪检查 |
| **发货记录** | 每次自动/手动发货的完整审计流水、失败原因、远端 consign/IM 状态、重试、CSV 导出 |
| **定时任务** | 内置 8 类任务（自动发货、商品同步、订单同步、消息同步、账号健康、日志清理、失败重试、通知重试）、可视化启停/改间隔/立即执行/执行历史 |
| **操作日志** | 中间件自动记录全部变更接口，支持搜索、状态过滤、统计、清理与 CSV 导出 |
| **通知设置** | Webhook / 飞书 / ServerChan / 邮件/短信网关渠道，事件规则与模板，发送日志与重试 |
| **系统设置** | 站点信息、安全策略、数据保留、备份与清理、系统运行信息 |
| **管理员页面** | 管理员 CRUD、角色（super_admin/admin/viewer）、状态、密码修改、登录日志、会话管理 |
| **闲鱼搜索分析** | 关键词实时搜索、中位价/均价/分位价、性价比指数与定价建议 |

---

## 技术栈

| 层次 | 选择 |
|------|------|
| 语言 | Rust 2024 Edition |
| Web | axum 0.8 + tokio |
| 数据库 | SQLite（sqlx） |
| 序列化 | serde / serde_json |
| 配置 | toml |
| 错误处理 | anyhow + thiserror |
| 前端 | 单页管理后台（Tailwind CDN + 原生 JS 模块） |
| 外部调用 | curl（闲鱼 MTOP / IM / 通知 Webhook） |

---

## 目录结构

```
AutoDeliver/
├── Cargo.toml
├── config.toml / config.example.toml
├── migrations/
│   ├── 001_init.sql              # 商品、卡密、订单、闲鱼账号/商品
│   └── 002_admin_modules.sql     # 管理员、消息、发货记录、定时任务、日志、通知、设置
├── docs/
│   ├── auto-delivery-plan.md
│   ├── learning_plan.md
│   ├── admin-modules-api.md      # 管理后台模块数据库与 API 契约
│   └── sync-and-automation.md    # 商品/消息/订单/自动发货同步机制
├── src/
│   ├── main.rs                   # 启动、迁移、调度器
│   ├── config.rs / error.rs
│   ├── audit.rs                  # 操作日志中间件
│   ├── scheduler.rs              # 定时任务调度器
│   ├── notify.rs                 # 通知发送
│   ├── db/                       # 数据访问层（无 mod.rs）
│   ├── handlers/                 # HTTP 接口层（无 mod.rs）
│   ├── models/                   # 数据结构
│   └── platform/                 # 闲鱼 MTOP / 搜索 / IM 适配
├── static/
│   ├── index.html                # 管理后台壳层
│   ├── modules-ops.js            # 在线消息 / 自动发货 / 发货记录
│   └── modules-system.js         # 定时任务 / 日志 / 通知 / 系统 / 管理员
└── data/                         # SQLite 数据库与备份（git 忽略）
```

> 项目禁止使用 `mod.rs`，统一 Rust 2018+ 模块文件组织。

---

## 快速开始

### 环境要求

- Rust 1.85+（Edition 2024）
- `curl`（闲鱼接口与通知发送依赖）
- 可选：SQLite CLI

### 启动

```bash
cp config.example.toml config.toml
# 按需修改数据库、端口、管理员账号与闲鱼配置
cargo run
```

打开 <http://127.0.0.1:8080>。

默认管理员（来自 `config.toml [auth]`，首次启动自动写入 `admins` 表）：

- 用户名：`admin`
- 密码：`admin123`

> 首次登录后请在「管理员」页面修改密码，并同步修改 `config.toml`。

### 接入闲鱼

进入「闲鱼账号」页面：

1. **手机扫码登录**：点击获取二维码，用闲鱼 App 扫码确认；
2. **Cookie 登录**：粘贴 `goofish.com` 登录后的完整 Cookie；
3. **OpenAPI**：填写闲鱼管家 AppKey / AppSecret / AccessToken。

登录后系统会自动验签、刷新 `_m_h5_tk`，并在「商品」页同步该账号的闲鱼商品。绑定卡密商品后，后台每 60 秒通过卖家订单列表导入「待发货」订单（不依赖买家是否发过消息），并在订单页展示；点击订单「重试」即可执行虚拟发货 + 卡密私信。IM 会话扫描仍作为有聊天记录订单的补充通道。

---

## Docker 部署

镜像里已包含 `curl`（闲鱼 MTOP / IM / 通知全靠外部 curl 进程）、`static/` 与 `migrations/`，
数据库落在 volume `/app/data`，容器重建不丢数据。

```bash
# 服务器上
mkdir -p /home/orangepi/autodeliver/data && cd /home/orangepi/autodeliver
cp config.toml.example config.toml    # 改掉 admin_password 与 session_token
docker compose up -d
curl -s http://127.0.0.1:8080/ -o /dev/null -w '%{http_code}\n'   # 200
```

`deploy/docker-compose.yml` 只把 8080 绑到 `127.0.0.1`（不暴露局域网），并通过 `autodeliver`
网络给同机的 AstrBot 容器使用：

```bash
docker network connect autodeliver astrbot
# AstrBot 插件里 base_url 填 http://autodeliver:8080
```

构建与分发：

| 场景 | 做法 |
|------|------|
| 本机 arm64 → 局域网服务器 | `./deploy/ship.sh`：buildx 构建 → `docker save \| ssh docker load` → compose 起服务 → 自动接网络 |
| x86 镜像 | 推到 `main` 后由 GitHub Actions 构建并推 `ghcr.io/makima04/autodeliver:latest`（`.github/workflows/docker.yml`，仅 `linux/amd64`） |

> ⚠️ `static/`、`migrations/` 是运行时按相对路径读取的，镜像里必须保留，别只拷二进制。
> 容器内 `config.toml` 的 `[server] host` 必须是 `0.0.0.0`，否则其他容器连不上。

---

## 自动发货流程

```
定时任务 xianyu_order_sync (默认 60s)
  → 卖家订单列表 mtop.taobao.idle.trade.merchant.sold.get
  → 只拉待发货（NOT_SHIP），按 orderId 幂等导入 orders
  → 找到本地绑定的卡密商品（未绑定则跳过并提示）
  → 订单页显示「待发货」，不自动扣码/发货
  → 用户点击「重试」或 IM 扫描命中后执行：
      原子抽取未使用兑换码（platform_order_id 幂等防重）
      → consign.dummy 标记闲鱼已发货
      → IM 发送卡密消息
      → 写入 delivery_records / operation_logs
      → 按通知规则推送成功/失败事件
```

管理后台「自动发货」页可调整扫描间隔、关闭远端发货/IM、设置消息模板、开启演练模式，并查看每个卡密商品的发货就绪状态。

---

## API 文档

完整接口、字段与页面契约见 [docs/admin-modules-api.md](docs/admin-modules-api.md)。

商品如何同步、消息如何同步、订单如何扫描、自动发货如何幂等防重，见
[docs/sync-and-automation.md](docs/sync-and-automation.md)。

主要接口分组：

- `/api/auth/*`、`/api/admins/*`
- `/api/xianyu/*`（账号、商品、搜索、扫码、发货扫描）
- `/api/products/*`、`/api/codes/*`
- `/api/orders/*`
- `/api/messages/*`
- `/api/auto-delivery/*`
- `/api/deliveries/*`
- `/api/scheduler/*`
- `/api/logs/*`
- `/api/notifications/*`
- `/api/settings`、`/api/system/*`

除登录、Webhook、静态资源外，新增管理接口均要求 `Authorization: Bearer <token>`。

---

## QQ Bot 集成（AstrBot 插件）

插件已独立成仓：[`astrbot_plugin_xianyu_autodeliver`](https://github.com/Makima04/astrbot_plugin_xianyu_autodeliver)，
把本平台接到 QQ：用 `/闲鱼登录` 登录管理端、`/闲鱼扫码` 扫码添加账号、`/闲鱼删除 <id>` 指定删除、
`/闲鱼搜索 switch` 实时搜索并由大模型总结后回传到 QQ。

插件只通过本项目的 REST API 工作（Bearer Token），不重复实现闲鱼协议。
AstrBot 插件页直接填上面的仓库地址即可安装。

---

## 验证

```bash
cargo check
cargo test
```

当前测试覆盖：卡密发放与库存不足、闲鱼订单幂等、消息解析/去重/未读、管理员密码/会话/权限、定时任务幂等与并发保护、通知模板与发送失败、自动发货就绪检查、数据库迁移。

---

## 许可证

MIT License
