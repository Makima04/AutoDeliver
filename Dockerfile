# syntax=docker/dockerfile:1

# ---- 构建阶段 ----
# 默认跟随 rust 官方最新稳定版；要复现旧版本可以 --build-arg RUST_IMAGE_TAG=1.88
ARG RUST_IMAGE_TAG=bookworm
FROM rust:${RUST_IMAGE_TAG} AS builder
WORKDIR /build

# 先只拷 Cargo 清单，用空壳 main.rs 把依赖编译一遍，命中 Docker 层缓存；
# 之后改业务代码不会重复下载/编译第三方 crate。
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src \
    && printf 'fn main() {}\n' > src/main.rs \
    && cargo build --release \
    && rm -rf src target/release/deps/learn_grammar* target/release/learn_grammar

COPY src ./src
COPY static ./static
RUN cargo build --release

# ---- 运行阶段 ----
FROM debian:bookworm-slim AS runtime

# curl 是硬依赖：闲鱼 MTOP / IM / 通知 Webhook 全部通过外部 curl 进程调用
# tzdata 用于让容器内时间和业务 SQL 的 localtime 对齐
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl tzdata \
    && rm -rf /var/lib/apt/lists/*

ENV TZ=Asia/Shanghai
WORKDIR /app

COPY --from=builder /build/target/release/learn_grammar /app/auto-deliver
# static/ 与 migrations/ 都是运行时从工作目录读的，必须一起进镜像
COPY static ./static
COPY migrations ./migrations
COPY config.example.toml ./config.example.toml

RUN mkdir -p /app/data
VOLUME ["/app/data"]

EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8080/ > /dev/null || exit 1

CMD ["/app/auto-deliver"]
