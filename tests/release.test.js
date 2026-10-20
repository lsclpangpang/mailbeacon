const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const R = require('../scripts/release.js');

const changelog = `# 更新记录

## 0.2.0

- 新功能甲
- 新功能乙

## 0.1.0

- 第一版
`;

test('the notes for a version are its changelog section', () => {
  assert.equal(R.section(changelog, '0.2.0'), '- 新功能甲\n- 新功能乙');
  assert.equal(R.section(changelog, '0.1.0'), '- 第一版');
  assert.equal(R.section(changelog, '0.3.0'), null);
});

test('release notes add install steps and the full diff link', () => {
  const notes = R.notes({ changelog, version: '0.2.0', previous: 'v0.1.0', repo: 'o/r' });
  assert.match(notes, /^## 更新内容\n\n- 新功能甲/);
  assert.match(notes, /xattr -dr com\.apple\.quarantine/);
  assert.match(notes, /https:\/\/github\.com\/o\/r\/compare\/v0\.1\.0\.\.\.v0\.2\.0/);
  assert.doesNotMatch(R.notes({ changelog, version: '0.1.0', previous: null, repo: 'o/r' }), /compare/);
  assert.throws(() => R.notes({ changelog, version: '0.3.0', previous: null, repo: 'o/r' }), /CHANGELOG\.md 里没有 0\.3\.0/);
});

function project(version = '0.2.0', lockVersion = version) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'mb-release-'));
  fs.mkdirSync(path.join(dir, 'src-tauri'));
  fs.writeFileSync(path.join(dir, 'src-tauri/tauri.conf.json'), JSON.stringify({ productName: 'MailBeacon', version }, null, 2) + '\n');
  fs.writeFileSync(path.join(dir, 'src-tauri/Cargo.toml'), `[package]\nname = "mailpulse-app"\nversion = "${version}"\n\n[dependencies]\nserde = { version = "1" }\n`);
  fs.writeFileSync(path.join(dir, 'Cargo.lock'), `[[package]]\nname = "cli"\nversion = "0.1.0"\n\n[[package]]\nname = "mailpulse-app"\nversion = "${lockVersion}"\n`);
  fs.writeFileSync(path.join(dir, 'package.json'), JSON.stringify({ name: 'mailbeacon', version }, null, 2) + '\n');
  fs.writeFileSync(path.join(dir, 'package-lock.json'), JSON.stringify({ name: 'mailbeacon', version, packages: { '': { name: 'mailbeacon', version } } }, null, 2) + '\n');
  fs.writeFileSync(path.join(dir, 'CHANGELOG.md'), changelog);
  return dir;
}

test('the check passes when every version agrees and the changelog has it', () => {
  assert.deepEqual(R.check(project()), []);
});

test('the check names each file whose version differs and a missing changelog entry', () => {
  const problems = R.check(project('0.3.0', '0.2.0'));
  assert.ok(problems.some((p) => /Cargo\.lock/.test(p)), problems.join('\n'));
  assert.ok(problems.some((p) => /CHANGELOG\.md 里没有 0\.3\.0/.test(p)), problems.join('\n'));
});

test('bumping changes every version field and adds a changelog stub', () => {
  const dir = project();
  R.bump(dir, '0.3.0');
  assert.equal(JSON.parse(fs.readFileSync(path.join(dir, 'src-tauri/tauri.conf.json'))).version, '0.3.0');
  assert.match(fs.readFileSync(path.join(dir, 'src-tauri/Cargo.toml'), 'utf8'), /^version = "0\.3\.0"$/m);
  assert.match(fs.readFileSync(path.join(dir, 'src-tauri/Cargo.toml'), 'utf8'), /serde = \{ version = "1" \}/, '依赖的版本不动');
  const lock = fs.readFileSync(path.join(dir, 'Cargo.lock'), 'utf8');
  assert.match(lock, /name = "mailpulse-app"\nversion = "0\.3\.0"/);
  assert.match(lock, /name = "cli"\nversion = "0\.1\.0"/, '其他包不动');
  const pkgLock = JSON.parse(fs.readFileSync(path.join(dir, 'package-lock.json')));
  assert.equal(pkgLock.version, '0.3.0');
  assert.equal(pkgLock.packages[''].version, '0.3.0');
  assert.match(fs.readFileSync(path.join(dir, 'CHANGELOG.md'), 'utf8'), /# 更新记录\n\n## 0\.3\.0\n\n- （待补充）\n\n## 0\.2\.0/);
  assert.deepEqual(R.check(dir).filter((p) => !/待补充/.test(p)), [], '除了还没写更新内容，各处版本都已一致');
  assert.throws(() => R.bump(dir, '1.0'), /版本号/);
});

test('a version whose notes are still the placeholder does not pass the check', () => {
  const dir = project();
  R.bump(dir, '0.3.0');
  assert.ok(R.check(dir).some((p) => /待补充/.test(p)));
  const file = path.join(dir, 'CHANGELOG.md');
  fs.writeFileSync(file, fs.readFileSync(file, 'utf8').replace('- （待补充）', '- 新功能丙'));
  assert.deepEqual(R.check(dir), []);
});

test('bumping only touches the version lines and keeps hand formatting', () => {
  const dir = project();
  const conf = path.join(dir, 'src-tauri/tauri.conf.json');
  const hand = '{\n  "productName": "MailBeacon",\n  "version": "0.2.0",\n  "app": { "trafficLightPosition": { "x": 20, "y": 24 } },\n  "bundle": { "targets": ["app", "dmg"], "version": "keep" }\n}\n';
  fs.writeFileSync(conf, hand);
  R.bump(dir, '0.3.0');
  assert.equal(fs.readFileSync(conf, 'utf8'), hand.replace('"version": "0.2.0"', '"version": "0.3.0"'));
});

test('the cargo version is only taken from the [package] table', () => {
  const dir = project();
  const cargo = path.join(dir, 'src-tauri/Cargo.toml');
  fs.writeFileSync(cargo, '[package]\nname = "mailpulse-app"\nversion.workspace = true\n\n[dependencies.foo]\nversion = "9.9.9"\n');
  assert.throws(() => R.bump(dir, '0.3.0'), /Cargo\.toml/);
  assert.match(fs.readFileSync(cargo, 'utf8'), /version = "9\.9\.9"/, '依赖的版本不能被改');
});

test('a changelog without the expected layout is reported instead of silently skipped', () => {
  const dir = project();
  fs.writeFileSync(path.join(dir, 'CHANGELOG.md'), '# 更新记录\r\n\r\n## 0.2.0\r\n\r\n- 甲\r\n');
  assert.equal(R.section(fs.readFileSync(path.join(dir, 'CHANGELOG.md'), 'utf8'), '0.2.0'), '- 甲');
  R.bump(dir, '0.3.0');
  assert.match(fs.readFileSync(path.join(dir, 'CHANGELOG.md'), 'utf8'), /## 0\.3\.0/, 'CRLF 也能加上');
  fs.writeFileSync(path.join(dir, 'CHANGELOG.md'), 'no heading here\n');
  assert.throws(() => R.bump(dir, '0.4.0'), /CHANGELOG\.md/);
});

test('the previous tag is the newest one below the released version', () => {
  const tags = ['v0.3.0', 'v0.2.0', 'v0.1.0', 'v0.10.0'];
  assert.equal(R.previousTag(tags, '0.2.0'), 'v0.1.0', '重发旧版本时不能和更新的版本比较');
  assert.equal(R.previousTag(tags, '0.4.0'), 'v0.3.0');
  assert.equal(R.previousTag(tags, '0.11.0'), 'v0.10.0', '按版本号比较，不是按字符串');
  assert.equal(R.previousTag([], '0.1.0'), null);
  assert.equal(R.previousTag(['v0.1.0', 'nightly'], '0.1.0'), null);
});

test('bumping refuses to change anything besides the version fields', () => {
  const dir = project();
  const lockFile = path.join(dir, 'package-lock.json');
  // 根条目没有 version：不能把下一个包的版本改掉
  fs.writeFileSync(lockFile, JSON.stringify({ name: 'mailbeacon', version: '0.2.0', packages: { '': { name: 'mailbeacon' }, 'node_modules/x': { version: '9.9.9' } } }, null, 2) + '\n');
  assert.throws(() => R.bump(dir, '0.3.0'), /package-lock\.json/);
  assert.match(fs.readFileSync(lockFile, 'utf8'), /9\.9\.9/);
  assert.equal(JSON.parse(fs.readFileSync(path.join(dir, 'package.json'))).version, '0.2.0', '一处不对就什么都不写');
});
