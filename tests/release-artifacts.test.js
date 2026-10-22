const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const A = require('../scripts/release-artifacts.js');
const sha = 'a'.repeat(40);
function fixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mb-release-artifacts-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const dirs = Object.fromEntries(['macOS', 'Windows'].map(platform => {
    const dir = path.join(root, platform);
    fs.mkdirSync(dir);
    const filename = platform === 'macOS' ? 'MailBeacon_1.2.3_universal.dmg' : 'MailBeacon_1.2.3_x64-setup.exe';
    fs.writeFileSync(path.join(dir, filename), `${platform} installer bytes`);
    return [platform, { dir, filename }];
  }));
  const record = (platform, options = {}) => A.record({ directory: dirs[platform].dir, platform, version: '1.2.3', sha, ...options });
  const verify = () => A.verify({ directory: root, version: '1.2.3', sha });
  return { root, dirs, record, verify };
}

test('release verifies both installers against one version and source commit before publishing', t => {
  const f = fixture(t);
  for (const platform of Object.keys(f.dirs)) f.record(platform);
  assert.deepEqual(f.verify().map(x => path.basename(x)), [f.dirs.macOS.filename, f.dirs.Windows.filename]);
});

test('release refuses an absent platform, empty installer, duplicate installer or mismatched version', t => {
  const f = fixture(t);
  f.record('macOS');
  assert.throws(f.verify, /Windows|manifest/);
  assert.throws(() => f.record('Windows', { version: '2.0.0' }), /version|版本/);
  fs.writeFileSync(path.join(f.dirs.Windows.dir, f.dirs.Windows.filename), '');
  assert.throws(() => f.record('Windows'), /empty|空/);
  fs.writeFileSync(path.join(f.dirs.Windows.dir, f.dirs.Windows.filename), 'windows');
  fs.writeFileSync(path.join(f.dirs.Windows.dir, 'MailBeacon_1.2.3_second_x64-setup.exe'), 'windows');
  assert.throws(() => f.record('Windows'), /exactly one|一个/);
});

test('release refuses a different source commit or changed installer bytes', t => {
  const f = fixture(t);
  f.record('macOS');
  f.record('Windows', { sha: 'b'.repeat(40) });
  assert.throws(f.verify, /commit|提交/);
  f.record('Windows');
  fs.appendFileSync(path.join(f.dirs.Windows.dir, f.dirs.Windows.filename), 'tampered');
  assert.throws(f.verify, /digest|校验/);
});

test('release refuses manifest path traversal or platform/file substitution', t => {
  const f = fixture(t);
  f.record('macOS'); f.record('Windows');
  const manifestPath = path.join(f.dirs.Windows.dir, 'manifest.json');
  const manifest = JSON.parse(fs.readFileSync(manifestPath));
  fs.writeFileSync(manifestPath, JSON.stringify({ ...manifest, filename: '../macOS/' + f.dirs.macOS.filename }));
  assert.throws(f.verify, /filename|文件名/);
  fs.writeFileSync(manifestPath, JSON.stringify({ ...manifest, platform: 'macOS' }));
  assert.throws(f.verify, /platform|平台/);
});
