// Bind every installer to the exact prepared source commit before combining a release.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const platforms = ['macOS', 'Windows'];
const digest = file => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const pattern = (platform, version) => new RegExp(`^MailBeacon_${version.replace(/\./g, '\\.')}_(?:universal\\.dmg|x64-setup\\.exe)$`);
function validate({ platform, version, sha }) {
  if (!platforms.includes(platform)) throw new Error('Unknown installer platform');
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error('Invalid version');
  if (!/^[a-f0-9]{40}$/.test(sha)) throw new Error('Invalid source commit');
}
function filenameMatches(filename, platform, version) {
  const suffix = platform === 'macOS' ? '_universal.dmg' : '_x64-setup.exe';
  return path.basename(filename) === filename && pattern(platform, version).test(filename) && filename.endsWith(suffix);
}
/** Record the built installer's version, source commit and content digest beside it. */
function record({ directory, platform, version, sha }) {
  validate({ platform, version, sha });
  const extension = platform === 'macOS' ? '.dmg' : '.exe';
  const files = fs.readdirSync(directory).filter(name => name.endsWith(extension));
  if (files.length !== 1) throw new Error(`Expected exactly one ${platform} installer`);
  const filename = files[0];
  if (!filenameMatches(filename, platform, version)) throw new Error('Installer filename/version does not match release');
  const file = path.join(directory, filename);
  if (!fs.lstatSync(file).isFile() || fs.statSync(file).size === 0) throw new Error('Installer is empty or not a regular file');
  const manifest = { platform, version, sha, filename, sha256: digest(file) };
  fs.writeFileSync(path.join(directory, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n');
  return manifest;
}
/** Verify both build outputs have the prepared commit/version and unchanged bytes. */
function verify({ directory, version, sha }) {
  return platforms.map(platform => {
    validate({ platform, version, sha });
    const dir = path.join(directory, platform);
    const manifest = JSON.parse(fs.readFileSync(path.join(dir, 'manifest.json'), 'utf8'));
    if (manifest.platform !== platform) throw new Error('Installer platform does not match manifest');
    if (manifest.version !== version) throw new Error('Installer version does not match release');
    if (manifest.sha !== sha) throw new Error('Installer source commit does not match release');
    if (!filenameMatches(manifest.filename, platform, version)) throw new Error('Invalid installer filename');
    const file = path.join(dir, manifest.filename);
    if (!fs.lstatSync(file).isFile() || fs.statSync(file).size === 0) throw new Error('Installer is empty or not a regular file');
    if (digest(file) !== manifest.sha256) throw new Error('Installer digest 校验失败');
    return file;
  });
}
module.exports = { record, verify };
if (require.main === module) {
  const [command, directory, platform] = process.argv.slice(2);
  const params = { directory, platform, version: process.env.RELEASE_VERSION, sha: process.env.RELEASE_SHA };
  try {
    if (command === 'record') record(params);
    else if (command === 'verify') console.log(verify(params).join('\n'));
    else throw new Error('Usage: release-artifacts.js record <directory> <macOS|Windows> | verify <directory>');
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
}
