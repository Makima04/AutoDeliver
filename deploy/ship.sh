#!/usr/bin/env bash
# 本机构建 arm64 镜像 → 局域网直传服务器 → docker compose 启动 → 把 AstrBot 接进同一网络。
#
# 用法：
#   ./deploy/ship.sh                  # 默认 root@192.168.1.161
#   SERVER=root@10.0.0.2 ./deploy/ship.sh
#   PLATFORM=linux/amd64 SERVER=root@x86-host ./deploy/ship.sh
#
# 之所以不推 registry：本机是 arm64，服务器也是 arm64，局域网点对点传比走 GitHub 快一个数量级。
set -euo pipefail

SERVER="${SERVER:-root@192.168.1.161}"
REMOTE_DIR="${REMOTE_DIR:-/home/orangepi/autodeliver}"
IMAGE="${IMAGE:-autodeliver:local}"
PLATFORM="${PLATFORM:-linux/arm64}"
ASTRBOT_CONTAINER="${ASTRBOT_CONTAINER:-astrbot}"
# 宿主机侧端口（容器内始终是 8080，AstrBot 走容器名不受影响）
AUTODELIVER_PORT="${AUTODELIVER_PORT:-18080}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# 某些受控环境会拦 buildx 在 ~/.docker 下写活动文件，换个目录更稳
export BUILDX_CONFIG="${BUILDX_CONFIG:-/tmp/buildx-config}"

echo "==> [1/5] 构建镜像 ${IMAGE}（${PLATFORM}）"
mkdir -p "$BUILDX_CONFIG"
docker buildx build --platform "$PLATFORM" -t "$IMAGE" --load "$ROOT"

echo "==> [2/5] 直传镜像到 $SERVER"
docker save "$IMAGE" | ssh "$SERVER" 'docker load'

echo "==> [3/5] 同步 compose 与配置模板"
ssh "$SERVER" "mkdir -p '$REMOTE_DIR/data'"
scp -q "$ROOT/deploy/docker-compose.yml" "$SERVER:$REMOTE_DIR/docker-compose.yml"
scp -q "$ROOT/deploy/config.toml.example" "$SERVER:$REMOTE_DIR/config.toml.example"
# 镜像 tag / 宿主机端口写进 .env，之后手工 docker compose 命令不用再带环境变量
ssh "$SERVER" "cat > '$REMOTE_DIR/.env' <<EOF
AUTODELIVER_IMAGE=$IMAGE
AUTODELIVER_PORT=$AUTODELIVER_PORT
AUTODELIVER_BIND=${AUTODELIVER_BIND:-0.0.0.0}
EOF"
ssh "$SERVER" "cd '$REMOTE_DIR' && test -f config.toml || cp config.toml.example config.toml"

echo "==> [4/5] 启动 / 更新容器"
ssh "$SERVER" "cd '$REMOTE_DIR' && docker compose up -d"

echo "==> [5/5] 把 $ASTRBOT_CONTAINER 接进 autodeliver 网络"
ssh "$SERVER" "docker network connect autodeliver '$ASTRBOT_CONTAINER' 2>/dev/null || true"

echo
echo "==> 健康检查"
ssh "$SERVER" "docker ps --filter name=autodeliver --format '{{.Names}} {{.Status}}'"
ssh "$SERVER" "curl -fsS -o /dev/null -w '宿主机 → autodeliver: HTTP %{http_code}\\n' http://127.0.0.1:${AUTODELIVER_PORT:-18080}/" \
  || echo "⚠️ 宿主机访问 ${AUTODELIVER_PORT:-18080} 失败，看日志：ssh $SERVER 'docker logs --tail 50 autodeliver'"
ssh "$SERVER" "docker exec '$ASTRBOT_CONTAINER' python -c \"import urllib.request;print('astrbot → autodeliver: HTTP', urllib.request.urlopen('http://autodeliver:8080/', timeout=5).status)\"" \
  || echo "⚠️ AstrBot 容器还连不上，检查网络：docker network inspect autodeliver"

cat <<EOF

完成。接下来：
  1. 管理端：http://<服务器IP>:${AUTODELIVER_PORT}/  （容器内仍是 8080）
  2. 在服务器 $REMOTE_DIR/config.toml 里改掉 admin_password / session_token，然后
     ssh $SERVER "cd $REMOTE_DIR && docker compose restart"
  3. AstrBot 插件配置里 base_url 填：http://autodeliver:8080
     （api_token 可填 config.toml 的 session_token，免登录）
EOF
