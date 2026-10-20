// 版本与发布说明管理。
//   node scripts/release.js check            各处版本号一致、CHANGELOG.md 有当前版本（CI 用）
//   node scripts/release.js notes [上一个tag|auto]  打印当前版本的发布说明（auto：从 git tag 里找上一个版本）
//   node scripts/release.js version 0.3.0    改所有版本号，并在 CHANGELOG.md 里加一节待补充
"use strict";

const fs = require("node:fs");
const path = require("node:path");

const SEMVER = /^\d+\.\d+\.\d+$/;
/** `version` 命令加的占位内容；没换成真正的更新内容前不能发布。 */
const PLACEHOLDER = "（待补充）";
const FILES = {
  tauri: "src-tauri/tauri.conf.json",
  cargo: "src-tauri/Cargo.toml",
  lock: "Cargo.lock",
  pkg: "package.json",
  pkgLock: "package-lock.json",
  changelog: "CHANGELOG.md",
};

const read = (dir, file) => fs.readFileSync(path.join(dir, file), "utf8");
const write = (dir, file, text) => fs.writeFileSync(path.join(dir, file), text);
const escape = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** CHANGELOG.md 里某个版本那一节的内容（不含标题）。 */
function section(changelog, version) {
  const lines = changelog.split("\n");
  const start = lines.findIndex((l) => new RegExp(`^##\\s+v?${escape(version)}\\s*$`).test(l));
  if (start < 0) return null;
  let end = lines.findIndex((l, i) => i > start && /^##\s/.test(l));
  if (end < 0) end = lines.length;
  return lines.slice(start + 1, end).join("\n").trim();
}

const parse = (v) => v.split(".").map(Number);
const older = (a, b) => { const [x, y] = [parse(a), parse(b)]; for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] < y[i]; return false; };

/** 比 `version` 小的最新一个 tag（按版本号比较）；重发旧版本时也不会和更新的版本比较。 */
function previousTag(tags, version) {
  const below = tags.map((t) => t.trim()).filter((t) => /^v\d+\.\d+\.\d+$/.test(t) && older(t.slice(1), version));
  below.sort((a, b) => (older(a.slice(1), b.slice(1)) ? 1 : -1));
  return below[0] || null;
}

/** Cargo.toml 的 [package] 表（到下一个表为止）。 */
function packageTable(cargo) {
  const m = cargo.match(/^\[package\][^\n]*\n((?:(?!\[)[^\n]*\n?)*)/m);
  return m ? { start: m.index, text: m[0] } : null;
}
const PACKAGE_VERSION = /^version\s*=\s*"([^"]+)"/m;

/** GitHub Release 的说明：更新内容（来自 CHANGELOG）、安装方法、完整差异链接。 */
function notes({ changelog, version, previous, repo }) {
  const body = section(changelog, version);
  if (!body) throw new Error(`CHANGELOG.md 里没有 ${version} 这一节，先写好更新内容再发布`);
  const parts = [
    `## 更新内容\n\n${body}`,
    [
      "## 安装",
      "",
      "1. 下载下面的 `MailBeacon_*_universal.dmg`（通用版，Apple Silicon 和 Intel 都能用），把 MailBeacon 拖进「应用程序」",
      "2. 没有 Apple 开发者证书签名，第一次打开会被拦截，在终端执行一次：",
      "   ```sh",
      "   xattr -dr com.apple.quarantine /Applications/MailBeacon.app",
      "   ```",
      "   或者先双击打开一次，再到「系统设置 → 隐私与安全性」里点「仍要打开」",
      "3. 添加邮箱时填写授权码，不是网页登录密码",
    ].join("\n"),
  ];
  if (previous) parts.push(`**完整变更**：https://github.com/${repo}/compare/${previous}...v${version}`);
  return parts.join("\n\n") + "\n";
}

function versions(dir) {
  const cargo = packageTable(read(dir, FILES.cargo))?.text.match(PACKAGE_VERSION)?.[1];
  const lock = read(dir, FILES.lock).match(/name = "mailpulse-app"\nversion = "([^"]+)"/)?.[1];
  const pkgLock = JSON.parse(read(dir, FILES.pkgLock));
  return {
    [FILES.tauri]: JSON.parse(read(dir, FILES.tauri)).version,
    [FILES.cargo]: cargo,
    [FILES.lock]: lock,
    [FILES.pkg]: JSON.parse(read(dir, FILES.pkg)).version,
    [`${FILES.pkgLock}（version）`]: pkgLock.version,
    [`${FILES.pkgLock}（packages[""].version）`]: pkgLock.packages?.[""]?.version,
  };
}

/** 发布前的检查；返回发现的问题（空表示没问题）。发布版本以 tauri.conf.json 为准。 */
function check(dir) {
  const all = versions(dir);
  const version = all[FILES.tauri];
  const problems = [];
  if (!SEMVER.test(version || "")) problems.push(`${FILES.tauri} 的版本号「${version}」不是 x.y.z 格式`);
  for (const [file, v] of Object.entries(all)) {
    if (v !== version) problems.push(`${file} 的版本是 ${v ?? "（没找到）"}，应为 ${version}（以 ${FILES.tauri} 为准）`);
  }
  const body = section(read(dir, FILES.changelog), version);
  if (!body) problems.push(`CHANGELOG.md 里没有 ${version} 这一节`);
  else if (body.includes(PLACEHOLDER)) problems.push(`CHANGELOG.md 里 ${version} 的更新内容还是「${PLACEHOLDER}」，请写好再发布`);
  return problems;
}

/** 把所有版本号改成 `version`，并在 CHANGELOG.md 最前面加一节（已有就不加）。
 * 只改版本号所在的那一行，不重新排版文件；任何一处找不到都报错，不会只改一半。 */
function bump(dir, version) {
  if (!SEMVER.test(version)) throw new Error(`版本号应为 x.y.z，例如 0.3.0：${version}`);
  const edits = {};
  // JSON：只换顶层（两格缩进）的 "version"，改完再解析一遍确认
  const topLevel = (file) => {
    const text = read(dir, file);
    const next = text.replace(/^( {2}"version":\s*)"[^"]*"/m, `$1"${version}"`);
    if (next === text && JSON.parse(text).version !== version) throw new Error(`${file} 里没找到顶层的 "version"`);
    return next;
  };
  edits[FILES.tauri] = topLevel(FILES.tauri);
  edits[FILES.pkg] = topLevel(FILES.pkg);
  let lock = topLevel(FILES.pkgLock);
  const root = lock.indexOf('"": {');
  if (root >= 0) {
    const head = lock.slice(0, root), tail = lock.slice(root);
    lock = head + tail.replace(/("version":\s*)"[^"]*"/, `$1"${version}"`);
  }
  edits[FILES.pkgLock] = lock;
  // 改完后核对：除了版本字段以外内容完全不变，版本字段确实是新版本
  for (const file of [FILES.tauri, FILES.pkg, FILES.pkgLock]) {
    const before = JSON.parse(read(dir, file)), after = JSON.parse(edits[file]);
    const expected = structuredClone(before);
    expected.version = version;
    if (file === FILES.pkgLock && expected.packages?.[""]) {
      if (!("version" in expected.packages[""])) throw new Error(`${file} 的根条目（packages[""]）没有 version`);
      expected.packages[""].version = version;
    }
    if (JSON.stringify(after) !== JSON.stringify(expected)) throw new Error(`${file} 的版本号位置和预期不一致，请手动修改`);
  }
  const cargo = read(dir, FILES.cargo);
  const table = packageTable(cargo);
  if (!table || !PACKAGE_VERSION.test(table.text)) throw new Error(`${FILES.cargo} 的 [package] 里没有 version = "x.y.z"`);
  edits[FILES.cargo] = cargo.slice(0, table.start) + table.text.replace(PACKAGE_VERSION, `version = "${version}"`) + cargo.slice(table.start + table.text.length);
  const cargoLock = read(dir, FILES.lock);
  edits[FILES.lock] = cargoLock.replace(/(name = "mailpulse-app"\r?\nversion = )"[^"]+"/, `$1"${version}"`);
  if (edits[FILES.lock] === cargoLock && !new RegExp(`name = "mailpulse-app"\\r?\\nversion = "${escape(version)}"`).test(cargoLock)) throw new Error(`${FILES.lock} 里没找到 mailpulse-app`);
  const changelog = read(dir, FILES.changelog);
  if (!section(changelog, version)) {
    const m = changelog.match(/^# [^\r\n]*(\r?\n)\r?\n/);
    if (!m) throw new Error("CHANGELOG.md 应以「# 标题」和一个空行开头，才能在最前面加新版本");
    const eol = m[1];
    edits[FILES.changelog] = changelog.slice(0, m[0].length) + `## ${version}${eol}${eol}- ${PLACEHOLDER}${eol}${eol}` + changelog.slice(m[0].length);
  }
  for (const [file, text] of Object.entries(edits)) write(dir, file, text);
}

module.exports = { section, notes, check, bump, previousTag };

if (require.main === module) {
  const root = path.resolve(__dirname, "..");
  const [cmd, arg] = process.argv.slice(2);
  try {
    if (cmd === "check") {
      const problems = check(root);
      for (const p of problems) console.error(`✗ ${p}`);
      if (problems.length) process.exit(1);
      console.log(`✓ 版本 ${versions(root)[FILES.tauri]}，各处一致，CHANGELOG.md 已有这一节`);
    } else if (cmd === "notes") {
      const version = versions(root)[FILES.tauri];
      const repo = process.env.GITHUB_REPOSITORY || "lsclpangpang/mailbeacon";
      const previous = arg === "auto"
        ? previousTag(require("node:child_process").execFileSync("git", ["tag", "-l", "v*"], { cwd: root, encoding: "utf8" }).split("\n"), version)
        : arg || null;
      process.stdout.write(notes({ changelog: read(root, FILES.changelog), version, previous, repo }));
    } else if (cmd === "version") {
      bump(root, arg || "");
      console.log(`已改为 ${arg}。记得填写 CHANGELOG.md 里的更新内容，并运行 cargo check 确认 Cargo.lock。`);
    } else {
      console.error("用法：node scripts/release.js check | notes [上一个tag] | version <x.y.z>");
      process.exit(2);
    }
  } catch (e) {
    console.error(`✗ ${e.message}`);
    process.exit(1);
  }
}
