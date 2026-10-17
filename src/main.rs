mod audit;
mod config;
mod db;
mod error;
mod handlers;
mod models;
mod notify;
mod order_sync;
mod platform;
mod scheduler;

use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = config::load()?;
    let pool = db::init(&config.database.url).await?;

    // 初始化管理员、内置定时任务与通知渠道（全部幂等）
    db::admin::ensure_seed_admin(&pool, &config.auth).await?;
    db::scheduler::ensure_default_tasks(&pool).await?;
    db::notification::ensure_defaults(&pool).await?;

    // 启动后台统一调度器：自动发货扫描、商品同步、消息同步、清理任务等均可在「定时任务」页面配置
    let scheduler_pool = pool.clone();
    let scheduler_xianyu = config.xianyu.clone();
    tokio::spawn(async move {
        scheduler::run_loop(scheduler_pool, scheduler_xianyu).await;
    });

    // 启动友好提示：配置了自动同步时，启动后延迟触发一次商品/消息同步由调度器负责
    if config.xianyu.enabled {
        let warm_pool = pool.clone();
        let warm_xianyu = config.xianyu.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(15)).await;
            let _ =
                handlers::xianyu::run_auto_deliver_scan_cycle(&warm_pool, &warm_xianyu, None).await;
        });
    }

    let app = handlers::router(pool, config.auth, config.xianyu);
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("服务启动在 http://{}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}
