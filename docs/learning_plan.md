# 学习计划

## 1.项目骨架+配置读取
Cargo.toml: workspace配置、依赖声明
src/main.rs: fn main() 调用配置模块
src/config.rs: struct + serde 反序列化，反序列化是将配置文件中的数据转换为 Rust 结构体实例

## 2.引入自定义错误
src/error.rs: thiserror 定义 ConfigError 错误类型、IoError 错误类型
src/config.rs: 把anyhow 换成具体 Result<T,ConfigError>

## 3.数据库最小闭环(1张表 + 1个查询)
migrations/001_init.sql: 初始化数据库，创建 Product 表
src/models/product.rs: 实现 Product 结构体的数据库操作
src/db.rs: sqlx 连接 SQLite，写一个get_product函数

## 4.扩展模型 + 状态枚举
migrations/001_init.sql: 增加redeem_code表、order表
src/models/redeem_code.rs: enum 定义状态
src/models/order.rs: 综合运用：关联Product表、RedeemCode表
更新 src/db.rs: 增加兑换码查询、订单查询

## 5.模块拆分（重构）
src/db/mod.rs: 模块入口、公共导出，定义数据库操作的接口
src/db/product.rs: 把产品相关DB操作迁移过来
src/db/redeem_code.rs: 把兑换码操作迁移过来
src/db/order.rs: 把订单操作迁移过来
更新 src/main.rs: 用新模块路径串起完整流程


