const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const R = require('../scripts/release-retention.js');

function release(n, extra = {}) {
  const version = `0.${n}.0`;
  return { id: n, tag_name: `v${version}`, draft: false, prerelease: false,
    published_at: '2026-10-23T00:00:00Z', updated_at: '2026-10-23T00:00:00Z',
    assets: ['universal.dmg', 'x64-setup.exe'].map(suffix => ({ name: `MailBeacon_${version}_${suffix}`, state: 'uploaded', size: 10 })), ...extra };
}
const releases = n => Array.from({ length: n }, (_, i) => release(i + 1));
const plan = (rows, options = {}) => R.planReleases(rows, { keep: 5, currentTag: 'v0.8.0', latestId: 8, ...options });

test('keeps five highest stable versions regardless of API order or re-publication date', () => {
  const rows = releases(12).reverse(); rows.find(r => r.id === 1).published_at = '2099-01-01T00:00:00Z';
  const p = plan(rows, { currentTag: 'v0.12.0', latestId: 12 });
  assert.deepEqual(p.keep.map(r => r.tag_name), ['v0.12.0','v0.11.0','v0.10.0','v0.9.0','v0.8.0']);
  assert.deepEqual(p.remove.map(r => r.id), [1,2,3,4,5,6,7]);
});

test('drafts, prereleases and unrelated tags are never cleanup candidates', () => {
  const rows = releases(8).concat(release(99, { draft: true }), release(98, { prerelease: true }), release(97, { tag_name: 'nightly' }), release(96, { tag_name: 'v1.0.0-beta.1' }));
  const p = plan(rows);
  assert.deepEqual(p.remove.map(r => r.id), [1,2,3]);
  assert.equal(p.ignored.length, 4);
});

test('five or fewer stable releases do not delete anything', () => {
  for (const n of [1,4,5]) assert.deepEqual(plan(releases(n), { currentTag: `v0.${n}.0`, latestId: n }).remove, []);
});

test('invalid retention, duplicate identity, ambiguous versions and malformed lists fail closed', () => {
  for (const keep of [0, -1, 1.2, NaN, '5']) assert.throws(() => plan(releases(8), { keep }));
  assert.throws(() => plan({}));
  assert.throws(() => plan(releases(8).concat(release(8))));
  assert.throws(() => plan(releases(8).concat(release(20, { tag_name: 'v0.8.0' }))));
});

test('current published version and GitHub latest must both remain among the five kept releases', () => {
  assert.throws(() => plan(releases(8), { currentTag: 'v0.1.0' }));
  assert.throws(() => plan(releases(8), { latestId: 1 }));
  assert.throws(() => plan(releases(8), { latestId: 90 }));
  assert.throws(() => plan(releases(8), { currentTag: 'v0.9.0' }));
});

test('no cleanup until the current release is public and both matching installers are fully uploaded', () => {
  const corrupt = [r => r.draft = true, r => r.prerelease = true, r => r.published_at = null,
    r => r.assets.pop(), r => r.assets[0].size = 0, r => r.assets[0].state = 'starter',
    r => r.assets[1].name = 'MailBeacon_0.7.0_x64-setup.exe'];
  for (const change of corrupt) { const rows = releases(8); change(rows[7]); assert.throws(() => plan(rows)); }
});

function service(rows = releases(8), hooks = {}) {
  let stored = structuredClone(rows);
  const calls = [];
  const api = async (method, endpoint) => {
    calls.push([method, endpoint]);
    const override = hooks.before?.({ method, endpoint, calls, rows: stored });
    if (override !== undefined) return override;
    if (method === 'DELETE') { stored = stored.filter(r => !endpoint.endsWith('/' + r.id)); return null; }
    if (endpoint.endsWith('/latest')) return structuredClone(stored.find(r => r.id === rows.at(-1).id));
    const page = /\?per_page=100&page=(\d+)$/.exec(endpoint);
    if (page) return structuredClone(stored.slice((Number(page[1]) - 1) * 100, Number(page[1]) * 100));
    const id = Number(endpoint.split('/').at(-1));
    return structuredClone(stored.find(r => r.id === id));
  };
  const run = options => R.runCleanup({ repository: 'owner/mailbeacon', currentTag: rows.at(-1).tag_name, keep: 5, api, ...options });
  return { calls, run, rows: () => stored, deletes: () => calls.filter(([m]) => m === 'DELETE') };
}

test('default dry run lists a plan without sending any mutation', async () => {
  const f = service(); const result = await f.run();
  assert.equal(result.plan.remove.length, 3); assert.deepEqual(result.deleted, []);
  assert.ok(f.calls.every(([method]) => method === 'GET'));
});

test('apply deletes only release IDs, retains five, never deletes tags, and is idempotent', async () => {
  const f = service();
  assert.deepEqual((await f.run({ apply: true })).deleted.map(r => r.id), [1,2,3]);
  assert.deepEqual(f.deletes().map(([,p]) => p), [1,2,3].map(id => `repos/owner/mailbeacon/releases/${id}`));
  assert.equal(f.rows().length, 5);
  assert.deepEqual((await f.run({ apply: true })).deleted, []);
});

test('all API pages are fetched before the first possible deletion', async () => {
  const f = service(releases(105)); await f.run();
  assert.ok(f.calls.some(([,p]) => p.endsWith('page=2')));
  assert.equal(f.deletes().length, 0);
});

test('pagination, latest and malformed API failures never cause partial-plan deletion', async () => {
  for (const suffix of ['page=2', '/latest']) {
    const f = service(releases(105), { before: ({ endpoint }) => { if (endpoint.endsWith(suffix)) throw Error('API unavailable'); } });
    await assert.rejects(f.run({ apply: true }), /API unavailable/); assert.equal(f.deletes().length, 0);
  }
  const f = service(releases(8), { before: ({ endpoint }) => endpoint.includes('page=') ? {} : undefined });
  await assert.rejects(f.run({ apply: true })); assert.equal(f.deletes().length, 0);
});

test('a changed candidate is not deleted and cleanup stops', async () => {
  const f = service(releases(8), { before: ({ endpoint, rows }) => endpoint.endsWith('/releases/1') ? { ...rows[0], draft: true } : undefined });
  await assert.rejects(f.run({ apply: true })); assert.equal(f.deletes().length, 0);
});

test('a retained release disappearing during cleanup cannot leave fewer than five versions', async () => {
  const f = service(releases(8), { before: ({ method, endpoint, rows }) => {
    if (method === 'GET' && endpoint.includes('page=') && rows.length === 7) rows.splice(rows.findIndex(r => r.id === 7), 1);
  } });
  await f.run({ apply: true });
  assert.equal(f.rows().length, 5); assert.deepEqual(f.deletes().map(([,p]) => p.split('/').at(-1)), ['1','2']);
});

test('delete failures stop the run instead of continuing to remove more releases', async () => {
  const f = service(releases(8), { before: ({ method }) => { if (method === 'DELETE') throw Error('permission denied'); } });
  await assert.rejects(f.run({ apply: true }), /permission denied/); assert.equal(f.deletes().length, 1);
});

test('release workflow cleans only after publication and reports cleanup failures separately', () => {
  const yaml = fs.readFileSync(path.join(__dirname, '../.github/workflows/release.yml'), 'utf8');
  const cleanup = yaml.indexOf('id: cleanup');
  assert.ok(cleanup > yaml.indexOf('gh release view "$tag" --json url,tagName,assets'));
  const step = yaml.slice(yaml.lastIndexOf('      - name:', cleanup));
  assert.match(step, /continue-on-error: true/);
  assert.match(step, /node scripts\/release-retention\.js apply/);
  assert.match(step, /RELEASE_KEEP_COUNT: ['"]?5/);
  assert.match(step, /steps\.cleanup\.outcome == 'failure'/);
  assert.doesNotMatch(step, /if:.*always\(\)/);
});
