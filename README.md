# 自动发货平台 (Auto-Delivery Platform)

## 项目简介

本项目为虚拟商品卖家提供自动化发货系统，支持兑换码自动发放、Steam 礼物自动赠送等功能，实现 7×24 小时无人值守发货。

主要面向淘宝、闲鱼等平台的个人卖家或小团队，日均订单量 10-200 单。

---

## 目录结构

```
auto-delivery/
├── Cargo.toml              ← 已创建
├── config.example.toml     ← 已创建
├── src/
│   ├── main.rs             ← 程序入口
│   ├── config.rs           ← 配置管理
│   ├── error.rs            ← 统一错误类型
│   ├── db.rs               ← 数据库模块声明
│   ├── db/
│   │   ├── mod.rs          ← 已废弃，用 db.rs 代替
│   │   └── ...
│   ├── models.rs           ← 数据模型模块声明
│   └── models/
│       ├── product.rs
│       ├── redeem_code.rs
│       └── order.rs
└── migrations/
    └── 001_init.sql
```

---

## 技术栈

| 技术 | 用途 |
|------|------|
| Rust (2024 Edition) | 开发语言 |
| axum | Web 后端框架 |
| tokio | 异步运行时 |
| sqlx | 数据库操作（编译期 SQL 校验） |
| SQLite / PostgreSQL | 数据存储 |
| serde + serde_json | JSON 序列化 |
| reqwest | HTTP 客户端（调用外部 API） |
| tracing | 日志记录 |
| toml | 配置文件解析 |

---

## 快速开始

### 1. 环境要求

- Rust 1.78+（安装：[https://rustup.rs](https://rustup.rs)）
- （可选）SQLite 客户端，用于直接查看数据库

### 2. 运行自动发货服务

```bash
cd auto-delivery

# 复制并编辑配置文件
cp config.example.toml config.toml
# 打开 config.toml，填入你的数据库路径、API 密钥等

# 运行
cargo run
```

### 3. 运行语法学习示例

```bash
# 在根目录执行
cargo run
```

---

## 文档索引

| 文档 | 说明 |
|------|------|
| [项目规划](docs/auto-delivery-plan.md) | 需求分析、系统架构、技术选型、开发计划 |
| API 文档 | 接口定义（待补充） |
| 数据库设计 | 表结构说明（待补充） |
| 部署指南 | 生产环境部署（待补充） |

---

## 开发计划

- [x] 项目规划文档
- [ ] 数据库设计与初始化
- [ ] 核心数据模型（Product、Order、RedeemCode）
- [ ] 平台适配层（淘宝订单轮询）
- [ ] 发货引擎（兑换码发放）
- [ ] 管理后台 REST API
- [ ] Web 管理界面
- [ ] Steam 机器人集成
- [ ] 测试与部署

---

## 许可证

MIT License
