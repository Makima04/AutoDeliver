use axum::extract::State;
use sqlx::SqlitePool;

// (State(pool): State<SqlitePool>) 是 axum 的一种提取器语法
// axum extractor + rust 模式匹配的组合写法
// State<SqlitePool> 是 axum 提供的一个提取器，用于从请求上下文中提取共享状态（在这里是数据库连接池）。
// State(pool) 是 Rust 的模式匹配语法，用于将提取到的状态绑定到一个变量 pool 上，以便在函数体内使用。

// 数据库查询
// sqlx::query_scalar查询单个标量值
// fetch_one方法用于执行查询并获取单个结果，如果查询没有结果或有多个结果会返回错误。
// unwrap_or(0) 是 Rust 的 Option 或 Result 类型的方法，用于处理可能的错误或空值。在这里，如果查询没有结果（例如表为空），它会返回默认值 0，而不是引发错误。
// format! 是 Rust 的一个宏，用于创建格式化字符串。在这个例子中，它将查询到的商品数量插入到字符串 "商品数量: {}" 中，生成最终的输出字符串。
pub async fn handler(State(pool): State<SqlitePool>) -> String {
    // 这里我们可以使用 `pool` 来访问数据库，进行查询等操作。
    // 例如，我们可以查询一个商品列表，或者执行其他数据库相关的逻辑。
    // 查数据库商品数量
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM products")
        .fetch_one(&pool)   
        .await
        .unwrap_or(0);
    format!("商品数量: {}", count)
}

