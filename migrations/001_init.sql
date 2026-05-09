-- ============================================
-- Migration: 001_init
-- Date: 2026-05-02
-- Description: 初始化数据库，创建商品表、发货模板表和兑换码表
-- ============================================

-- 开启外键约束（SQLite 默认关闭，必须显式开启）
PRAGMA foreign_keys = ON;

-- --------------------------------------------
-- INTEGER 数据类型，这个字段智能存整数
-- PRIMARY KEY 主键约束，这张表的唯一标识符
-- AUTOINCREMENT 自增约束
-- 
-- 数据类型
-- TEXT 存储文本数据
-- BOOLEAN 存储布尔值，SQLite 中实际存储为整数 0 (false) 和 1 (true)
-- DATETIME 存储日期和时间，SQLite 中通常以文本格式存储，格式为 'YYYY-MM-DD HH:MM:SS'
-- INTEGER 存储整数，适用于价格等数值数据，避免浮点数精度问题
--
-- 约束
-- NOT NULL 非空约束，字段必须有值
-- DEFAULT 默认值约束，如果插入数据时没有提供该字段的值，则使用默认值
-- UNIQUE 唯一约束，字段值必须唯一
-- PRIMARY KEY 主键约束，唯一标识表中的每一行，通常与 AUTOINCREMENT 一起使用
-- REFERENCES 外键约束，定义表之间的关系，指定关联的表和字段，以及删除时的级联行为
-- CHECK 约束，限制字段的取值范围，例如 template_type 和 status 字段
-- ON DELETE CASCADE 级联删除约束，当关联的主表记录被删除时，自动删除子表中的相关记录
--
-- 索引
-- 索引是一种数据库对象，可以加快查询速度，特别是在 WHERE 子句
-- 中使用的字段上创建索引可以显著提高查询性能。常见的索引类型包括单列索引和联合索引。
-- --------------------------------------------


-- --------------------------------------------
-- 商品表 (products)
-- 存储所有在售虚拟商品的基础信息
-- 发货方式由 product_templates 表决定，实现商品与发货逻辑解耦
-- --------------------------------------------
CREATE TABLE IF NOT EXISTS products (
    id INTEGER PRIMARY KEY AUTOINCREMENT,  -- 自增主键
    name TEXT NOT NULL,                     -- 商品名称
    price INTEGER NOT NULL DEFAULT 0,       -- 价格(单位:分)，避免浮点数精度问题
    description TEXT,                       -- 商品描述
    enabled BOOLEAN NOT NULL DEFAULT true,  -- 是否上架销售
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP  -- 创建时间
);

-- --------------------------------------------
-- 发货模板表 (product_templates)
-- 定义每个商品的发货方式，与商品一一对应
--
-- 支持的发货类型：
--   redeem_code : 从 redeem_codes 库存中分配一个唯一兑换码
--   file_link   : 发送 content 字段中的通用链接/内容给所有买家
--   steam_gift  : 通过 Steam Bot 赠送游戏礼物
-- --------------------------------------------
CREATE TABLE IF NOT EXISTS product_templates (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id INTEGER NOT NULL UNIQUE REFERENCES products(id) ON DELETE CASCADE,
                                            -- 外键: 所属商品，一对一关系
                                            -- 删除商品时级联删除模板
    template_type TEXT NOT NULL
        CHECK (template_type IN ('redeem_code', 'file_link', 'steam_gift')),
                                            -- 发货类型，数据库层限制取值范围
    content TEXT,                           -- 通用发货内容
                                            -- redeem_code 类型下为 NULL
                                            -- file_link 类型下存放链接地址
                                            -- steam_gift 类型下存放 Steam AppID
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- --------------------------------------------
-- 兑换码表 (redeem_codes)
-- 仅用于 template_type = 'redeem_code' 的商品
-- 存储兑换码库存，每个码唯一，关联到具体商品
-- --------------------------------------------
CREATE TABLE IF NOT EXISTS redeem_codes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id INTEGER NOT NULL REFERENCES products(id) ON DELETE CASCADE,
                                            -- 外键: 所属商品，删除商品时级联删除兑换码
    code TEXT NOT NULL UNIQUE,              -- 兑换码内容，全局唯一
    status TEXT NOT NULL DEFAULT 'unused'   -- 状态: unused(未使用) / sold(已售出) / used(已兑换)
        CHECK (status IN ('unused', 'sold', 'used')),
                                            -- 数据库层限制取值范围
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    used_at DATETIME                        -- 使用时间，NULL 表示未使用
);

-- --------------------------------------------
-- 订单表 (orders)
-- 记录每一次购买行为
-- redeem_code_id 为 NULL 表示该商品不需要兑换码（如 file_link 类型）
-- --------------------------------------------
CREATE TABLE IF NOT EXISTS orders (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id INTEGER NOT NULL REFERENCES products(id),
    redeem_code_id INTEGER REFERENCES redeem_codes(id),
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_orders_product_id
    ON orders(product_id);


-- --------------------------------------------
-- 索引
-- --------------------------------------------

-- 按商品查询兑换码（库存统计）
CREATE INDEX IF NOT EXISTS idx_codes_product_id
    ON redeem_codes(product_id);

-- 按状态查询（筛选可用/已用）
CREATE INDEX IF NOT EXISTS idx_codes_status
    ON redeem_codes(status);

-- 联合索引：查询某商品的可用库存（最常用场景）
CREATE INDEX IF NOT EXISTS idx_codes_product_status
    ON redeem_codes(product_id, status);

-- 发货模板表索引：通过商品 ID 查询发货方式
CREATE INDEX IF NOT EXISTS idx_templates_product_id
    ON product_templates(product_id);

-- ============================================
-- 测试数据
-- ============================================

-- 商品测试数据（仅当 products 表为空时插入）

INSERT INTO products (name, price, description, enabled)
SELECT * FROM (
    SELECT 'Steam 充值卡 10元', 1000, 'Steam 钱包充值码', 1
    UNION ALL SELECT 'Steam 充值卡 50元', 5000, 'Steam 钱包充值码', 1
    UNION ALL SELECT 'Netflix 月卡', 2900, '共享账号，30天', 1
    UNION ALL SELECT 'Spotify 年卡', 12000, 'Premium 一年订阅', 0
    UNION ALL SELECT 'ChatGPT Plus 月卡', 15000, '独立账号，30天', 1
    UNION ALL SELECT 'Office 365 年卡', 30000, '正版授权，5设备', 1
    UNION ALL SELECT 'Adobe 全家桶', 50000, 'Creative Cloud 全套', 0
    UNION ALL SELECT '游戏激活码-艾尔登法环', 29800, 'Steam 国区激活码', 1
    UNION ALL SELECT '游戏激活码-黑神话悟空', 26800, 'WeGame 激活码', 1
    UNION ALL SELECT 'Discord Nitro 月卡', 8000, '服务器提升特权', 1
)
WHERE (SELECT COUNT(*) FROM products) = 0;

-- 兑换码测试数据
INSERT INTO redeem_codes (product_id, code, status)
SELECT 1, 'STEAM-10-XXXX1', 'unused'
WHERE NOT EXISTS (SELECT 1 FROM redeem_codes WHERE code = 'STEAM-10-XXXX1');

INSERT INTO redeem_codes (product_id, code, status)
SELECT 1, 'STEAM-10-XXXX2', 'unused'
WHERE NOT EXISTS (SELECT 1 FROM redeem_codes WHERE code = 'STEAM-10-XXXX2');

INSERT INTO redeem_codes (product_id, code, status)
SELECT 2, 'STEAM-50-XXXX1', 'unused'
WHERE NOT EXISTS (SELECT 1 FROM redeem_codes WHERE code = 'STEAM-50-XXXX1');
