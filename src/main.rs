mod config;
mod db;
mod error;
mod models;
mod handlers;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = config::load()?;
    let pool = db::init(&config.database.url).await?;

    let app = handlers::router(pool);
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("服务启动在 http://{}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}
