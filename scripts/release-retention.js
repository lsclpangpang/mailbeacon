// Release retention only: never delete Git tags or Actions artifacts.
'use strict';
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');

function version(tag) {
  const match = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.exec(tag);
  return match ? match.slice(1).map(BigInt) : null;
}
function compare(a, b) {
  const av = version(a.tag_name), bv = version(b.tag_name);
  for (let i = 0; i < 3; i++) if (av[i] !== bv[i]) return av[i] > bv[i] ? -1 : 1;
  return 0;
}
function completeInstallers(release) {
  const v = release.tag_name.slice(1);
  return ['universal.dmg', 'x64-setup.exe'].every(suffix => {
    const assets = (release.assets || []).filter(a => a.name === `MailBeacon_${v}_${suffix}`);
    return assets.length === 1 && assets[0].state === 'uploaded' && Number.isSafeInteger(assets[0].size) && assets[0].size > 0;
  });
}
function planReleases(releases, { keep = 5, currentTag, latestId }) {
  if (!Number.isSafeInteger(keep) || keep < 1) throw Error('保留数量必须是正整数');
  if (!Array.isArray(releases) || !version(currentTag)) throw Error('无效的版本列表或当前版本');
  const ids = new Set(), tags = new Set();
  for (const r of releases) {
    if (!r || !Number.isSafeInteger(r.id) || r.id < 1 || typeof r.tag_name !== 'string'
      || ids.has(r.id) || tags.has(r.tag_name)) throw Error('版本列表存在无效或重复身份，停止清理');
    ids.add(r.id); tags.add(r.tag_name);
  }
  const stable = releases.filter(r => version(r.tag_name) && r.draft === false && r.prerelease === false
    && typeof r.published_at === 'string' && Number.isFinite(Date.parse(r.published_at))).sort(compare);
  const retained = stable.slice(0, keep);
  const current = retained.find(r => r.tag_name === currentTag);
  if (!current || !retained.some(r => r.id === latestId)) throw Error('当前版本或 GitHub latest 不在保留范围内，停止清理');
  if (!completeInstallers(current)) throw Error('当前版本的 macOS / Windows 安装包尚未完整发布，停止清理');
  return { keep: retained, remove: stable.slice(keep).reverse(), ignored: releases.filter(r => !stable.includes(r)) };
}

function ghApi(method, endpoint) {
  const output = execFileSync('gh', ['api', '--method', method, endpoint, '-H', 'Accept: application/vnd.github+json'],
    { encoding: 'utf8', maxBuffer: 32 * 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] });
  return output.trim() ? JSON.parse(output) : null;
}
function identity(r) {
  return JSON.stringify([r?.id, r?.tag_name, r?.draft, r?.prerelease, r?.published_at, r?.updated_at, r?.immutable]);
}
async function runCleanup({ repository, currentTag, keep = 5, apply = false, api = ghApi, onDelete = () => {} }) {
  if (!/^[A-Za-z0-9][A-Za-z0-9-]*\/[A-Za-z0-9_.-]+$/.test(repository || '') || ['.', '..'].includes(repository.split('/')[1])) {
    throw Error('无效的 GitHub 仓库名称');
  }
  if (typeof apply !== 'boolean') throw Error('apply 必须为布尔值');
  if (typeof onDelete !== 'function') throw Error('onDelete 必须为函数');
  const base = `repos/${repository}/releases`;
  const snapshot = async () => {
    const rows = [], seen = new Set();
    for (let page = 1; ; page++) {
      const batch = await api('GET', `${base}?per_page=100&page=${page}`);
      if (!Array.isArray(batch)) throw Error('GitHub 版本分页返回格式错误');
      for (const r of batch) {
        if (!r || seen.has(r.id)) throw Error('GitHub 版本分页出现重复身份');
        seen.add(r.id); rows.push(r);
      }
      if (batch.length < 100) break;
    }
    const latest = await api('GET', `${base}/latest`);
    if (!rows.some(r => r.id === latest?.id && r.tag_name === latest?.tag_name)) throw Error('GitHub latest 与版本列表不一致');
    return planReleases(rows, { keep, currentTag, latestId: latest.id });
  };
  const plan = await snapshot(), deleted = [];
  if (!apply) return { plan, deleted };
  for (const candidate of plan.remove) {
    // Another maintainer may change releases while this workflow is running.
    // Recompute retention before every deletion, rather than executing a stale list.
    const refreshed = await snapshot();
    const allowed = refreshed.remove.find(r => r.id === candidate.id);
    if (!allowed) continue;
    const actual = await api('GET', `${base}/${candidate.id}`);
    if (identity(actual) !== identity(candidate) || identity(actual) !== identity(allowed)) {
      throw Error(`版本 ${candidate.tag_name} 在清理期间发生变化，停止清理`);
    }
    const latest = await api('GET', `${base}/latest`);
    if (!refreshed.keep.some(r => r.id === latest?.id && r.tag_name === latest?.tag_name)) {
      throw Error('GitHub latest 在清理期间发生变化，停止清理');
    }
    const current = refreshed.keep.find(r => r.tag_name === currentTag);
    const confirmed = latest.id === current.id ? latest : await api('GET', `${base}/${current.id}`);
    if (identity(confirmed) !== identity(current) || !completeInstallers(confirmed)) {
      throw Error('当前版本或安装包在清理期间发生变化，停止清理');
    }
    await api('DELETE', `${base}/${candidate.id}`);
    deleted.push(candidate);
    await onDelete(candidate);
  }
  return { plan, deleted };
}

function report(result, apply) {
  const tags = rows => rows.map(r => r.tag_name).join('、') || '无';
  return [
    `Release 清理${apply ? '' : '预览（未删除）'}`,
    `计划保留：${tags(result.plan.keep)}`,
    `${apply ? '已删除' : '计划删除'}：${tags(apply ? result.deleted : result.plan.remove)}`,
    `跳过草稿、预发布及非标准版本：${tags(result.plan.ignored)}`,
    'Git 标签与源码历史保留。',
  ].join('\n');
}
module.exports = { planReleases, runCleanup, report };
if (require.main === module) {
  const [command = 'plan'] = process.argv.slice(2);
  const apply = command === 'apply';
  (async () => {
    if (!['plan', 'apply'].includes(command) || process.argv.length > 3) throw Error('用法：release-retention.js [plan|apply]');
    const result = await runCleanup({ repository: process.env.GITHUB_REPOSITORY,
      currentTag: `v${process.env.RELEASE_VERSION}`, keep: Number(process.env.RELEASE_KEEP_COUNT ?? 5), apply,
      onDelete: release => {
        const line = `已删除旧 Release：${release.tag_name}（ID ${release.id}），Git 标签保留`;
        console.log(line);
        if (process.env.GITHUB_STEP_SUMMARY) fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY, `\n- ${line}\n`);
      } });
    const text = report(result, apply);
    console.log(text);
    if (process.env.GITHUB_STEP_SUMMARY) fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY, `\n### Release 保留策略\n\n${text}\n`);
  })().catch(error => { console.error(error.message); process.exitCode = 1; });
}
