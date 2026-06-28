#!/usr/bin/env bash
# 同步本地知识库 markdown 文件到远程服务器
#
# 用法:
#   bash scripts/sync_knowledge.sh          # 执行同步
#   bash scripts/sync_knowledge.sh --dry-run # 仅预览差异
#
# 配置文件: scripts/deploy.conf (与 deploy-remote.sh 共用)
#   首次使用请从 deploy.conf.example 复制并修改。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DEPLOY_CONF="${SCRIPT_DIR}/deploy.conf"
DRY_RUN=""

# --- 参数解析 ---
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run)
      DRY_RUN="--dry-run"
      shift
      ;;
    -h|--help)
      echo "用法: bash scripts/sync_knowledge.sh [--dry-run]"
      echo ""
      echo "  同步本地知识库 markdown 文件到远程服务器。"
      echo "  配置文件: scripts/deploy.conf (与 deploy-remote.sh 共用)"
      echo "  首次使用请从 deploy.conf.example 复制并修改。"
      exit 0
      ;;
    *)
      echo "未知参数: $1"
      exit 1
      ;;
  esac
done

# --- 加载配置 ---
if [[ ! -f "$DEPLOY_CONF" ]]; then
  echo "[错误] 配置文件不存在: $DEPLOY_CONF"
  echo "  请从 deploy.conf.example 复制并填入实际值。"
  exit 1
fi
source "$DEPLOY_CONF"

# --- 校验 ---
if [[ -z "${REMOTE_HOST:-}" ]]; then
  echo "[错误] deploy.conf 缺少 REMOTE_HOST"
  exit 1
fi
if [[ -z "${REMOTE_PROJECT_DIR:-}" ]]; then
  echo "[错误] deploy.conf 缺少 REMOTE_PROJECT_DIR"
  exit 1
fi
if [[ ${#SYNC_MAP[@]} -eq 0 ]]; then
  echo "[错误] deploy.conf 缺少 SYNC_MAP 条目"
  exit 1
fi

# 知识库在远程项目目录下的固定位置
REMOTE_KBASE="${REMOTE_PROJECT_DIR}/runtime/config/knowledge"

# --- 执行同步 ---
RSYNC_FLAGS="-avz --progress"
if [[ -n "$DRY_RUN" ]]; then
  RSYNC_FLAGS="$RSYNC_FLAGS --dry-run"
  echo "=== 预览模式 (--dry-run) ==="
fi

OK=0
SKIP=0
FAIL=0

for ENTRY in "${SYNC_MAP[@]}"; do
  LOCAL_DIR="${ENTRY%%|*}"
  SUB_DIR="${ENTRY##*|}"

  echo ""
  echo ">>> 同步: ${LOCAL_DIR}/ -> ${REMOTE_HOST}:${REMOTE_KBASE}/${SUB_DIR}/"

  if [[ ! -d "$LOCAL_DIR" ]]; then
    echo "  [跳过] 本地目录不存在: $LOCAL_DIR"
    SKIP=$((SKIP + 1))
    continue
  fi

  if rsync ${RSYNC_FLAGS} \
    --include='*/' --include='*.md' --exclude='*' \
    "${LOCAL_DIR}/" "${REMOTE_HOST}:${REMOTE_KBASE}/${SUB_DIR}/"; then
    OK=$((OK + 1))
  else
    echo "  [失败] rsync 返回非零: ${LOCAL_DIR}"
    FAIL=$((FAIL + 1))
  fi
done

echo ""
echo "=== 同步完成: 成功 ${OK}, 跳过 ${SKIP}, 失败 ${FAIL} ==="
