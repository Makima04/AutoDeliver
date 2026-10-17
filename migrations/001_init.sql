-- ============================================
-- Migration: 001_init
-- Description: 初始化数据库表结构（无任何模拟测试数据）
-- ============================================

PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS system_meta (
    meta_key TEXT PRIMARY KEY,
    meta_value TEXT NOT NULL
);

-- 卡密商品表 (products) —— 在“卡密库存中心”创建、修改、删除，与闲鱼商品绑定后支持自动发货
CREATE TABLE IF NOT EXISTS products (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    price INTEGER NOT NULL DEFAULT 0,
    description TEXT,
    xianyu_item_id TEXT UNIQUE,
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- 发货模板表 (product_templates)
CREATE TABLE IF NOT EXISTS product_templates (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id INTEGER NOT NULL UNIQUE REFERENCES products(id) ON DELETE CASCADE,
    template_type TEXT NOT NULL
        CHECK (template_type IN ('redeem_code', 'file_link', 'steam_gift')),
    content TEXT,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- 兑换码库存表 (redeem_codes)
CREATE TABLE IF NOT EXISTS redeem_codes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id INTEGER NOT NULL REFERENCES products(id) ON DELETE CASCADE,
    code TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL DEFAULT 'unused'
        CHECK (status IN ('unused', 'sold', 'used')),
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    used_at DATETIME
);

-- 订单表 (orders)
CREATE TABLE IF NOT EXISTS orders (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id INTEGER NOT NULL REFERENCES products(id) ON DELETE CASCADE,
    redeem_code_id INTEGER REFERENCES redeem_codes(id) ON DELETE SET NULL,
    platform TEXT NOT NULL DEFAULT 'manual',
    platform_order_id TEXT,
    buyer_id TEXT,
    buyer_nickname TEXT,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- 闲鱼卖家账号表 (xianyu_accounts) —— 支持多账号管理，ID 从 1 开始连续分配
CREATE TABLE IF NOT EXISTS xianyu_accounts (
    id INTEGER PRIMARY KEY,
    auth_mode TEXT NOT NULL DEFAULT 'cookie'
        CHECK (auth_mode IN ('cookie', 'open_api')),
    seller_id TEXT,
    seller_nick TEXT,
    raw_cookie TEXT,
    m_h5_tk TEXT,
    app_key TEXT,
    app_secret TEXT,
    access_token TEXT,
    status TEXT NOT NULL DEFAULT 'connected',
    last_verified_at DATETIME,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- 闲鱼账号下的闲鱼商品表 (xianyu_items)
CREATE TABLE IF NOT EXISTS xianyu_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES xianyu_accounts(id) ON DELETE CASCADE,
    xianyu_item_id TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    price INTEGER NOT NULL DEFAULT 0,
    original_price INTEGER,
    stock INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL DEFAULT 'on_sale'
        CHECK (status IN ('on_sale', 'off_shelf')),
    category TEXT,
    description TEXT,
    image_url TEXT,
    local_product_id INTEGER REFERENCES products(id) ON DELETE SET NULL,
    synced_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_xianyu_items_account_id ON xianyu_items(account_id);
CREATE INDEX IF NOT EXISTS idx_orders_product_id ON orders(product_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_orders_platform_order_id
    ON orders(platform, platform_order_id) WHERE platform_order_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_products_xianyu_item_id ON products(xianyu_item_id);
CREATE INDEX IF NOT EXISTS idx_codes_product_id ON redeem_codes(product_id);
CREATE INDEX IF NOT EXISTS idx_codes_status ON redeem_codes(status);
CREATE INDEX IF NOT EXISTS idx_codes_product_status ON redeem_codes(product_id, status);
CREATE INDEX IF NOT EXISTS idx_templates_product_id ON product_templates(product_id);
