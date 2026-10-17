-- ============================================
-- Migration: 002_admin_modules
-- Description: 管理后台扩展模块（管理员、消息、自动发货、发货记录、定时任务、日志、通知、系统设置）
-- ============================================

PRAGMA foreign_keys = ON;

-- 管理员账号与登录会话
CREATE TABLE IF NOT EXISTS admins (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    display_name TEXT,
    role TEXT NOT NULL DEFAULT 'admin'
        CHECK (role IN ('super_admin', 'admin', 'viewer')),
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'disabled')),
    last_login_at DATETIME,
    last_login_ip TEXT,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS admin_sessions (
    token TEXT PRIMARY KEY,
    admin_id INTEGER NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    username TEXT NOT NULL,
    role TEXT NOT NULL,
    expires_at DATETIME NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_admin_sessions_admin_id ON admin_sessions(admin_id);
CREATE INDEX IF NOT EXISTS idx_admin_sessions_expires_at ON admin_sessions(expires_at);

CREATE TABLE IF NOT EXISTS admin_login_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    admin_id INTEGER,
    username TEXT NOT NULL,
    ip TEXT,
    user_agent TEXT,
    success BOOLEAN NOT NULL DEFAULT false,
    message TEXT,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_admin_login_logs_created_at ON admin_login_logs(created_at);

-- 闲鱼账号连接事件日志
CREATE TABLE IF NOT EXISTS xianyu_connection_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES xianyu_accounts(id) ON DELETE CASCADE,
    event TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'info'
        CHECK (status IN ('success', 'failed', 'info')),
    message TEXT,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_xianyu_connection_logs_account ON xianyu_connection_logs(account_id);
CREATE INDEX IF NOT EXISTS idx_xianyu_connection_logs_created_at ON xianyu_connection_logs(created_at);

-- 发货记录（每一次自动/手动发货尝试）
CREATE TABLE IF NOT EXISTS delivery_records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    order_id INTEGER,
    account_id INTEGER,
    xianyu_item_id TEXT,
    platform_order_id TEXT,
    product_id INTEGER,
    product_name TEXT,
    buyer_nickname TEXT,
    delivery_content TEXT,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('success', 'failed', 'pending')),
    source TEXT NOT NULL DEFAULT 'auto_scan'
        CHECK (source IN ('auto_scan', 'webhook', 'manual', 'api', 'retry')),
    remote_consign_status TEXT,
    remote_im_status TEXT,
    error_message TEXT,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_delivery_records_status ON delivery_records(status);
CREATE INDEX IF NOT EXISTS idx_delivery_records_account ON delivery_records(account_id);
CREATE INDEX IF NOT EXISTS idx_delivery_records_order ON delivery_records(order_id);
CREATE INDEX IF NOT EXISTS idx_delivery_records_created_at ON delivery_records(created_at);

-- 在线消息会话与消息
CREATE TABLE IF NOT EXISTS message_conversations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES xianyu_accounts(id) ON DELETE CASCADE,
    seller_nick TEXT,
    session_cid TEXT NOT NULL,
    buyer_id TEXT,
    buyer_nickname TEXT,
    xianyu_item_id TEXT,
    last_message TEXT,
    last_message_at DATETIME,
    unread_count INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'open'
        CHECK (status IN ('open', 'closed')),
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (account_id, session_cid)
);

CREATE INDEX IF NOT EXISTS idx_message_conversations_account ON message_conversations(account_id);
CREATE INDEX IF NOT EXISTS idx_message_conversations_last_at ON message_conversations(last_message_at);
CREATE INDEX IF NOT EXISTS idx_message_conversations_status ON message_conversations(status);

CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id INTEGER NOT NULL REFERENCES message_conversations(id) ON DELETE CASCADE,
    account_id INTEGER NOT NULL,
    session_cid TEXT NOT NULL,
    direction TEXT NOT NULL DEFAULT 'in'
        CHECK (direction IN ('in', 'out')),
    sender_id TEXT,
    sender_nick TEXT,
    content TEXT NOT NULL,
    msg_type TEXT NOT NULL DEFAULT 'text'
        CHECK (msg_type IN ('text', 'order_card', 'system')),
    order_id TEXT,
    remote_msg_id TEXT,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_messages_conversation ON messages(conversation_id);
CREATE INDEX IF NOT EXISTS idx_messages_created_at ON messages(created_at);

-- 定时任务与执行历史
CREATE TABLE IF NOT EXISTS scheduled_tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_key TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    interval_seconds INTEGER NOT NULL DEFAULT 60,
    enabled BOOLEAN NOT NULL DEFAULT true,
    last_run_at DATETIME,
    next_run_at DATETIME,
    last_status TEXT NOT NULL DEFAULT 'idle',
    last_message TEXT,
    last_duration_ms INTEGER NOT NULL DEFAULT 0,
    run_count INTEGER NOT NULL DEFAULT 0,
    fail_count INTEGER NOT NULL DEFAULT 0,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_enabled ON scheduled_tasks(enabled);
CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_next_run ON scheduled_tasks(next_run_at);

CREATE TABLE IF NOT EXISTS scheduled_task_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id INTEGER NOT NULL REFERENCES scheduled_tasks(id) ON DELETE CASCADE,
    task_key TEXT NOT NULL,
    status TEXT NOT NULL,
    message TEXT,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    started_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at DATETIME
);

CREATE INDEX IF NOT EXISTS idx_scheduled_task_runs_task ON scheduled_task_runs(task_id);
CREATE INDEX IF NOT EXISTS idx_scheduled_task_runs_started ON scheduled_task_runs(started_at);

-- 操作日志（由 audit 中间件自动写入）
CREATE TABLE IF NOT EXISTS operation_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    actor TEXT,
    action TEXT NOT NULL,
    method TEXT NOT NULL,
    path TEXT NOT NULL,
    resource_type TEXT,
    resource_id TEXT,
    status_code INTEGER NOT NULL DEFAULT 200,
    detail TEXT,
    ip TEXT,
    user_agent TEXT,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_operation_logs_created_at ON operation_logs(created_at);
CREATE INDEX IF NOT EXISTS idx_operation_logs_method ON operation_logs(method);
CREATE INDEX IF NOT EXISTS idx_operation_logs_status ON operation_logs(status_code);

-- 通知渠道、规则与发送日志
CREATE TABLE IF NOT EXISTS notification_channels (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    channel_type TEXT NOT NULL
        CHECK (channel_type IN ('webhook', 'feishu', 'serverchan', 'email', 'sms')),
    config_json TEXT NOT NULL DEFAULT '{}',
    enabled BOOLEAN NOT NULL DEFAULT false,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS notification_rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_id INTEGER NOT NULL REFERENCES notification_channels(id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT true,
    template TEXT,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (channel_id, event_type)
);

CREATE INDEX IF NOT EXISTS idx_notification_rules_event ON notification_rules(event_type);

CREATE TABLE IF NOT EXISTS notification_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_id INTEGER,
    event_type TEXT NOT NULL,
    title TEXT,
    content TEXT,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('sent', 'failed', 'pending')),
    response TEXT,
    error_message TEXT,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_notification_logs_created_at ON notification_logs(created_at);
CREATE INDEX IF NOT EXISTS idx_notification_logs_status ON notification_logs(status);

-- 系统设置键值表
CREATE TABLE IF NOT EXISTS system_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL DEFAULT '',
    label TEXT,
    group_name TEXT NOT NULL DEFAULT 'basic',
    description TEXT,
    value_type TEXT NOT NULL DEFAULT 'string'
        CHECK (value_type IN ('string', 'int', 'bool', 'json')),
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);
