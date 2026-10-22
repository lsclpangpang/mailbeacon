#!/usr/bin/env bash
set -euo pipefail

if [ "${GITHUB_EVENT_NAME:-}" != push ] || [ "${GITHUB_REF:-}" != refs/heads/main ]; then
  echo "版本回写仅允许 main 的 push 事件" >&2
  exit 1
fi
: "${RELEASE_DEPLOY_KEY:?缺少 release-main 环境的 RELEASE_DEPLOY_KEY}"
: "${RUNNER_TEMP:?}"
: "${GITHUB_REPOSITORY:?}"
: "${GITHUB_OUTPUT:?}"

umask 077
release_dir=$(mktemp -d "$RUNNER_TEMP/release-writeback.XXXXXX")
trap 'rm -rf "$release_dir"' EXIT
printf '%s\n' "$RELEASE_DEPLOY_KEY" > "$release_dir/key"
unset RELEASE_DEPLOY_KEY
# 从经过 HTTPS 验证的 GitHub API 取得官方 host keys；不信任即时 ssh-keyscan。
gh api meta --jq '.ssh_keys[] | "github.com " + .' > "$release_dir/known_hosts"
test -s "$release_dir/known_hosts"
release_ssh="ssh -F /dev/null -i \"$release_dir/key\" -o IdentitiesOnly=yes -o IdentityAgent=none -o BatchMode=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=\"$release_dir/known_hosts\""

git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
for attempt in 1 2 3; do
  if git diff --quiet HEAD --; then
    echo "sha=$(git rev-parse HEAD)" >> "$GITHUB_OUTPUT"
    exit 0
  fi
  # 专用凭据只提交 release.js 生成的版本文件与更新记录。
  git diff --name-only HEAD -- | while IFS= read -r file; do
    case "$file" in
      package.json|package-lock.json|Cargo.lock|src-tauri/Cargo.toml|src-tauri/tauri.conf.json|CHANGELOG.md) ;;
      *) echo "拒绝回写非版本文件：$file" >&2; exit 1 ;;
    esac
  done
  version=$(node -p "require('./src-tauri/tauri.conf.json').version")
  # Deploy key 推送会触发工作流；跳过新提交的 CI，当前发布仍继续测试和打包。
  git commit -qam "chore(release): v$version [skip ci]"
  if GIT_SSH_COMMAND="$release_ssh" git push -q "git@github.com:$GITHUB_REPOSITORY.git" HEAD:main; then
    echo "sha=$(git rev-parse HEAD)" >> "$GITHUB_OUTPUT"
    exit 0
  fi
  if [ "$attempt" = 3 ]; then break; fi
  # main 在计算期间前进时，在最新提交上重新规划，保留正常的非强制推送。
  git fetch -q origin main
  git reset -q --hard origin/main
  if [ -z "$(node scripts/release.js plan)" ]; then
    echo "最新的 main 上已经没有要发布的内容，不再发布" >&2
    exit 1
  fi
done
echo "推送版本号到 main 失败" >&2
exit 1
