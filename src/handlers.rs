use std::time::Instant;

use axum::{
    Router,
    extract::FromRef,
    middleware,
    routing::{delete, get, patch, post, put},
};
use sqlx::SqlitePool;

use crate::config::{AuthConfig, XianyuConfig};

pub mod account;
pub mod admin;
pub mod assets;
pub mod auth;
pub mod auto_delivery;
pub mod delivery;
pub mod hello;
pub mod message;
pub mod notification;
pub mod order;
pub mod product;
pub mod scheduler;
pub mod system;
pub mod xianyu;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub auth: AuthConfig,
    pub xianyu: XianyuConfig,
    pub started_at: Instant,
}

impl FromRef<AppState> for SqlitePool {
    fn from_ref(state: &AppState) -> Self {
        state.pool.clone()
    }
}

pub fn router(pool: SqlitePool, auth: AuthConfig, xianyu: XianyuConfig) -> Router {
    let audit_pool = pool.clone();
    let state = AppState {
        pool,
        auth,
        xianyu,
        started_at: Instant::now(),
    };
    // 鉴权中间件要在 with_state 把 state move 走之前留一份
    let guard_state = state.clone();

    Router::<AppState>::new()
        .route("/", get(hello::handler))
        .route("/assets/{*path}", get(assets::serve))
        // 管理后台登录与会话接口
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/me", get(auth::current_session))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/change-password", post(auth::change_password))
        // 卡密库存中心的卡密商品与兑换码管理接口
        .route(
            "/api/products",
            get(product::list_products).post(product::create_product),
        )
        .route("/api/products/stats", get(product::product_stats))
        .route(
            "/api/products/{id}",
            get(product::get_product)
                .put(product::update_product)
                .delete(product::delete_product),
        )
        .route(
            "/api/products/{id}/template",
            get(product::get_product_template),
        )
        .route("/api/products/{id}/stock", get(product::get_product_stock))
        .route(
            "/api/products/{id}/codes",
            get(product::list_product_codes).post(product::import_product_codes),
        )
        .route("/api/codes/id/{id}", delete(product::delete_code_by_id))
        .route("/api/codes/{code}", get(product::get_code_by_value))
        .route("/api/admin/clear_demo", post(product::clear_demo_data))
        // 订单管理接口
        .route(
            "/api/orders",
            get(order::list_orders).post(order::create_order),
        )
        .route("/api/orders/stats", get(order::order_stats))
        .route("/api/orders/sync", post(order::sync_orders))
        .route(
            "/api/orders/{id}",
            get(order::get_order).delete(order::delete_order),
        )
        .route("/api/orders/{id}/status", patch(order::update_order_status))
        .route("/api/orders/{id}/retry", post(order::retry_order))
        .route("/api/orders/{id}/resend", post(order::resend_order))
        // 闲鱼多账号管理、商品自动同步/增删改/绑定卡密商品、全网搜索与性价比分析接口
        .route("/api/xianyu/account", get(xianyu::get_account_status))
        .route("/api/xianyu/accounts", get(xianyu::list_accounts))
        .route("/api/xianyu/accounts/{id}", delete(xianyu::delete_account))
        .route(
            "/api/xianyu/accounts/{id}/verify",
            post(xianyu::verify_account_by_id),
        )
        .route(
            "/api/xianyu/accounts/{id}/items",
            get(xianyu::list_account_items).post(xianyu::create_account_item),
        )
        .route(
            "/api/xianyu/accounts/{id}/items/sync",
            post(xianyu::sync_account_items),
        )
        .route(
            "/api/xianyu/items/{item_id}",
            put(xianyu::update_item).delete(xianyu::delete_item),
        )
        .route(
            "/api/xianyu/items/{item_id}/price",
            patch(xianyu::update_item_price),
        )
        .route(
            "/api/xianyu/items/{item_id}/bind",
            put(xianyu::bind_item_product),
        )
        .route("/api/xianyu/search", post(xianyu::search_market_items))
        .route("/api/xianyu/qrcode/generate", post(xianyu::generate_qrcode))
        .route("/api/xianyu/qrcode/poll", post(xianyu::poll_qrcode))
        .route("/api/xianyu/login", post(xianyu::login_account))
        .route("/api/xianyu/verify", post(xianyu::verify_account))
        .route("/api/xianyu/logout", post(xianyu::logout_account))
        .route(
            "/api/xianyu/auto-deliver/scan",
            post(xianyu::scan_and_auto_deliver),
        )
        .route(
            "/api/xianyu/orders/consign",
            post(xianyu::consign_and_deliver_order),
        )
        .route(
            "/api/webhooks/xianyu/order",
            post(xianyu::handle_order_webhook),
        )
        // 新增管理模块
        .merge(account::router())
        .merge(admin::router())
        .merge(auto_delivery::router())
        .merge(delivery::router())
        .merge(message::router())
        .merge(notification::router())
        .merge(scheduler::router())
        .merge(system::router())
        .with_state(state)
        .layer(middleware::from_fn_with_state(
            guard_state,
            crate::guard::middleware,
        ))
        .layer(middleware::from_fn_with_state(
            audit_pool,
            crate::audit::middleware,
        ))
}
