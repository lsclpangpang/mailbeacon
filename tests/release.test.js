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
  fs.writeFileSync(path.join(dir, 'src-tauri/Cargo.toml'), `[package]\nname = "mailbeacon-app"\nversion = "${version}"\n\n[dependencies]\nserde = { version = "1" }\n`);
  fs.writeFileSync(path.join(dir, 'Cargo.lock'), `[[package]]\nname = "cli"\nversion = "0.1.0"\n\n[[package]]\nname = "mailbeacon-app"\nversion = "${lockVersion}"\n`);
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
  assert.match(lock, /name = "mailbeacon-app"\nversion = "0\.3\.0"/);
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
  fs.writeFileSync(cargo, '[package]\nname = "mailbeacon-app"\nversion.workspace = true\n\n[dependencies.foo]\nversion = "9.9.9"\n');
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

// ---------------------------------------------------------------------------
// 按 PR 标题自动发版：fix → 末位，feat → 中间位，带 ! → 首位，其他类型不发版
// ---------------------------------------------------------------------------

test('PR titles decide the kind of release', () => {
  const kinds = {
    'fix: 修复中文关键词搜不到旧邮件': 'patch',
    'fix(search): 中文搜索': 'patch',
    'perf: 列表滚动更流畅': 'patch',
    'feat: 会话模式': 'minor',
    'feat(compose)：模板支持变量': 'minor',
    'feat!: 改用新的配置格式': 'major',
    'fix(imap)!: 不再兼容旧缓存': 'major',
    'docs: 补充安装说明': null,
    'ci: 缓存 cargo': null,
    'chore: 升级依赖': null,
    'refactor: 拆分 lib.rs': null,
    'test: 补充搜索测试': null,
  };
  for (const [title, kind] of Object.entries(kinds)) {
    assert.equal(R.classify(title).bump, kind, title);
  }
  assert.equal(R.classify('fix: 修复中文关键词搜不到旧邮件').summary, '修复中文关键词搜不到旧邮件');
  assert.equal(R.classify('feat(compose)：模板支持变量').summary, '模板支持变量');
  for (const bad of ['修复中文关键词搜不到旧邮件', 'Fix: 大写', 'fix:没有空格', 'fix: ', 'feature: 新功能', 'bugfix: x']) {
    assert.throws(() => R.classify(bad), /PR 标题/, bad);
  }
});

test('the next version bumps one part and resets the ones after it', () => {
  assert.equal(R.nextVersion('0.2.0', 'patch'), '0.2.1');
  assert.equal(R.nextVersion('0.2.9', 'minor'), '0.3.0');
  assert.equal(R.nextVersion('0.2.9', 'major'), '1.0.0');
  assert.equal(R.nextVersion('1.4.2', 'patch'), '1.4.3');
});

const read = (dir, file) => fs.readFileSync(path.join(dir, file), 'utf8');

test('a fix merged after v0.2.0 is released as 0.2.1 with the PR title as its notes', () => {
  const dir = project();
  const version = R.plan(dir, { changes: [{ title: 'fix: 修复中文关键词搜不到旧邮件', number: 5 }], tags: ['v0.1.0', 'v0.2.0'] });
  assert.equal(version, '0.2.1');
  assert.equal(R.section(read(dir, 'CHANGELOG.md'), '0.2.1'), '- 修复中文关键词搜不到旧邮件（#5）');
  assert.deepEqual(R.check(dir), [], '各处版本一致，更新记录已写好');
});

test('features bump the middle part and breaking changes the first', () => {
  assert.equal(R.plan(project(), { changes: [{ title: 'feat: 会话模式', number: 7 }], tags: ['v0.2.0'] }), '0.3.0');
  assert.equal(R.plan(project(), { changes: [{ title: 'feat!: 新配置格式', number: 8 }], tags: ['v0.2.0'] }), '1.0.0');
});

test('the newest tag, not the checked-in version, is the base', () => {
  // 仓库里的版本号落后于 tag（比如发版提交还没合进来）时，也不能重复发同一个版本
  const dir = project('0.2.0');
  assert.equal(R.plan(dir, { changes: [{ title: 'fix: x', number: 1 }], tags: ['v0.2.0', 'v0.2.3', 'v0.10.0', 'junk'] }), '0.10.1');
});

test('docs, ci and other maintenance merges do not release and leave files alone', () => {
  const dir = project();
  const before = read(dir, 'CHANGELOG.md') + read(dir, 'src-tauri/tauri.conf.json');
  assert.equal(R.plan(dir, { changes: [{ title: 'docs: 补充说明', number: 9 }], tags: ['v0.2.0'] }), null);
  assert.equal(R.plan(dir, { changes: [{ title: '随便写的标题', number: 9 }], tags: ['v0.2.0'] }), null, '标题不合规的历史提交不发版，也不让发布失败');
  assert.equal(read(dir, 'CHANGELOG.md') + read(dir, 'src-tauri/tauri.conf.json'), before);
});

test('hand-written notes under 「未发布」 become the new version section', () => {
  const dir = project();
  fs.writeFileSync(path.join(dir, 'CHANGELOG.md'), changelog.replace('# 更新记录\n\n', '# 更新记录\n\n## 未发布\n\n- **搜索**：中文关键词能搜到几年前的邮件\n- **链接**：正文链接可以点开\n\n'));
  assert.equal(R.plan(dir, { changes: [{ title: 'fix: 搜索', number: 5 }], tags: ['v0.2.0'] }), '0.2.1');
  const text = read(dir, 'CHANGELOG.md');
  assert.doesNotMatch(text, /未发布/);
  assert.equal(R.section(text, '0.2.1'), '- **搜索**：中文关键词能搜到几年前的邮件\n- **链接**：正文链接可以点开');
  assert.deepEqual(R.check(dir), []);
});

test('a version raised by hand in the PR is released as is', () => {
  const dir = project();
  R.bump(dir, '0.5.0');
  fs.writeFileSync(path.join(dir, 'CHANGELOG.md'), read(dir, 'CHANGELOG.md').replace('- （待补充）', '- 大版本'));
  assert.equal(R.plan(dir, { changes: [{ title: 'fix: 小修', number: 3 }], tags: ['v0.2.0'] }), '0.5.0');
  assert.equal(R.section(read(dir, 'CHANGELOG.md'), '0.5.0'), '- 大版本', '手写的说明不被覆盖');
});

test('PR titles with a trailing merge number are understood', () => {
  // 没有 PR 信息时退回用 squash 合并的提交标题，例如「fix: 搜索 (#5)」
  assert.equal(R.classify('fix: 修复搜索 (#5)').summary, '修复搜索');
  // GitHub 自动生成的撤销 PR 标题
  assert.deepEqual({ ...R.classify('Revert "feat: 会话模式"') }, { type: 'revert', breaking: false, bump: 'patch', summary: '撤销：会话模式' });
});

test('every change since the last tag counts: the biggest bump wins and each gets a line', () => {
  const dir = project();
  const version = R.plan(dir, { tags: ['v0.2.0'], changes: [
    { title: 'fix: 修复搜索', number: 5 },
    { title: 'docs: 改说明', number: 6 },
    { title: 'feat: 会话模式', number: 7 },
  ] });
  assert.equal(version, '0.3.0');
  assert.equal(R.section(read(dir, 'CHANGELOG.md'), '0.3.0'), '- 修复搜索（#5）\n- 会话模式（#7）');
});

test('a release that was committed but never tagged is redone with the later changes', () => {
  // 上一次发布把 0.2.1 提交回了 main 但打包失败（没有 tag），之后又合并了一个新功能
  const dir = project();
  R.plan(dir, { tags: ['v0.2.0'], changes: [{ title: 'fix: 修复搜索', number: 5 }] });
  const version = R.plan(dir, { tags: ['v0.2.0'], changes: [{ title: 'fix: 修复搜索', number: 5 }, { title: 'feat: 会话模式', number: 7 }] });
  assert.equal(version, '0.3.0');
  const text = read(dir, 'CHANGELOG.md');
  assert.doesNotMatch(text, /## 0\.2\.1/);
  assert.equal(R.section(text, '0.3.0'), '- 修复搜索（#5）\n- 会话模式（#7）');
  assert.deepEqual(R.check(dir), []);
  // 再算一遍结果不变（重跑工作流）
  assert.equal(R.plan(dir, { tags: ['v0.2.0'], changes: [{ title: 'fix: 修复搜索', number: 5 }, { title: 'feat: 会话模式', number: 7 }] }), '0.3.0');
  assert.equal(R.section(read(dir, 'CHANGELOG.md'), '0.3.0'), '- 修复搜索（#5）\n- 会话模式（#7）');
});

test('an empty 「未发布」 section is dropped instead of left behind', () => {
  const dir = project();
  fs.writeFileSync(path.join(dir, 'CHANGELOG.md'), changelog.replace('# 更新记录\n\n', '# 更新记录\n\n## 未发布\n\n'));
  assert.equal(R.plan(dir, { tags: ['v0.2.0'], changes: [{ title: 'fix: 搜索', number: 5 }] }), '0.2.1');
  const text = read(dir, 'CHANGELOG.md');
  assert.doesNotMatch(text, /未发布/);
  assert.equal(R.section(text, '0.2.1'), '- 搜索（#5）');
});

test('a commit whose PR cannot be looked up falls back to its squash subject', () => {
  const commits = [
    { sha: 'c', subject: 'feat: 会话模式 (#7)' },
    { sha: 'b', subject: 'chore(release): v0.2.1' },
    { sha: 'a', subject: '修复搜索 (#5)' },
  ];
  // 接口刚合并时查不到会返回空字段，而不是 null
  const lookups = [];
  const lookup = (sha) => {
    lookups.push(sha);
    return { a: { number: 5, title: 'fix: 修复搜索' }, c: { number: null, title: null } }[sha];
  };
  assert.deepEqual(R.changesFrom(commits, lookup).map((c) => ({ ...c })), [
    { number: '7', title: 'feat: 会话模式 (#7)' },
    { number: 5, title: 'fix: 修复搜索' },
  ]);
  assert.deepEqual(lookups, ['c', 'c', 'a'], '查不到时再试一次；自动发版提交不查');
});

test('the same PR reached through several commits is counted once', () => {
  const commits = [{ sha: 'b', subject: 'x' }, { sha: 'a', subject: 'y' }];
  const changes = R.changesFrom(commits, () => ({ number: 9, title: 'fix: 同一个 PR' }));
  assert.equal(changes.length, 1);
});
