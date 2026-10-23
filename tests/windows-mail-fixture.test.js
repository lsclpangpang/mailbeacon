const test = require('node:test');
const assert = require('node:assert/strict');
const tls = require('node:tls');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const readline = require('node:readline');
const { once } = require('node:events');
const { spawn } = require('node:child_process');

async function fixture(t) {
  const { startMailFixture } = await import('../scripts/windows-mail-fixture.mjs');
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'mailbeacon-fixture-'));
  const server = await startMailFixture({ reportPath: path.join(directory, 'report.json') });
  t.after(async () => { await server.close(); fs.rmSync(directory, { recursive: true, force: true }); });
  return server;
}

async function client(t, config) {
  const socket = tls.connect({ host: config.host, port: config.imapPort, ca: fs.readFileSync(config.caPath) });
  const input = readline.createInterface({ input: socket, crlfDelay: Infinity });
  const lines = input[Symbol.asyncIterator]();
  t.after(() => { input.close(); socket.destroy(); });
  await once(socket, 'secureConnect');
  assert.match((await lines.next()).value, /^\* OK /);
  return {
    socket,
    async command(tag, command) {
      socket.write(`${tag} ${command}\r\n`);
      const response = [];
      for (;;) {
        const next = await lines.next();
        assert.equal(next.done, false, 'server must return a tagged response');
        response.push(next.value);
        if (next.value.startsWith(`${tag} `)) return response;
      }
    },
    async line() { return (await lines.next()).value; },
  };
}

test('loopback TLS fixture requires its explicit CA and advertises only loopback endpoints', { timeout: 10000 }, async t => {
  const server = await fixture(t);
  assert.equal(server.config.host, '127.0.0.1');
  assert.equal(server.config.smtpHost, '127.0.0.1');
  assert.match(server.config.email, /@example\.invalid$/);
  const untrusted = tls.connect({ host: server.config.host, port: server.config.imapPort });
  t.after(() => untrusted.destroy());
  const [error] = await once(untrusted, 'error');
  assert.match(error.code, /SELF_SIGNED|UNABLE_TO_VERIFY/);
  const connection = await client(t, server.config);
  assert.equal(connection.socket.authorized, true);
});

test('failed login does not authenticate or leak credentials; correct login opens an empty inbox', { timeout: 10000 }, async t => {
  const server = await fixture(t), c = await client(t, server.config);
  const { email, password, invalidPassword } = server.config;
  assert.match((await c.command('a1', `LOGIN "${email}" "${invalidPassword}"`)).at(-1), /NO \[AUTHENTICATIONFAILED\] fixture-auth-rejected/);
  assert.match((await c.command('a2', 'SELECT INBOX')).at(-1), /NO \[AUTHENTICATIONREQUIRED\]/);
  assert.equal(server.report().selectAccepted, 0);
  assert.match((await c.command('a3', `LOGIN "${email}" "${password}"`)).at(-1), /OK /);
  assert.match((await c.command('a4', 'CAPABILITY')).join('\n'), /IMAP4rev1.*IDLE/);
  assert.match((await c.command('a5', 'ID ("name" "MailBeacon")')).join('\n'), /\* ID /);
  assert.match((await c.command('a6', 'LIST "" "*"')).join('\n'), /\* LIST .*"INBOX"/);
  assert.match((await c.command('a6b', 'SELECT missing')).at(-1), /NO unknown-mailbox/);
  assert.equal(server.report().selectAccepted, 0);
  const selected = await c.command('a7', 'SELECT "INBOX"');
  assert.ok(selected.includes('* 0 EXISTS'));
  assert.match(selected.join('\n'), /UIDVALIDITY 1/);
  assert.match((await c.command('a8', 'STATUS "INBOX" (MESSAGES UNSEEN UIDNEXT UIDVALIDITY)')).join('\n'), /MESSAGES 0 UNSEEN 0 UIDNEXT 1 UIDVALIDITY 1/);
  assert.deepEqual(await c.command('a9', 'UID SEARCH ALL'), ['* SEARCH', 'a9 OK SEARCH completed']);
  assert.deepEqual(await c.command('a10', 'UID FETCH 1:* (UID FLAGS)'), ['a10 OK FETCH completed']);
  assert.match((await c.command('a11', 'LOGOUT')).join('\n'), /\* BYE/);
  const report = server.report();
  assert.equal(report.authRejected, 1);
  assert.equal(report.authAccepted, 1);
  assert.equal(report.commands.SELECT, 3);
  assert.equal(report.selectAccepted, 1);
  assert.ok(report.stages.includes('selected'));
  const saved = fs.readFileSync(server.config.reportPath, 'utf8');
  for (const value of [email, password, invalidPassword]) assert.ok(!saved.includes(value));
});

test('IDLE waits for DONE and unsupported or malformed commands cannot bypass authentication', { timeout: 10000 }, async t => {
  const server = await fixture(t), c = await client(t, server.config);
  assert.match((await c.command('b0', 'AUTHENTICATE PLAIN')).at(-1), /NO \[AUTHENTICATIONREQUIRED\]/);
  assert.match((await c.command('b1', 'LOGIN {100}')).at(-1), /BAD /);
  await c.command('b2', `LOGIN "${server.config.email}" "${server.config.password}"`);
  c.socket.write('b3 IDLE\r\n');
  assert.equal(await c.line(), '+ idling');
  c.socket.write('DONE\r\n');
  assert.equal(await c.line(), 'b3 OK IDLE completed');
  assert.match((await c.command('b4', 'DELETE INBOX')).at(-1), /BAD unsupported-command/);
  assert.equal(server.report().commands.IDLE, 1);
  assert.equal(server.report().commands.DONE, 1);
});

test('CLI publishes complete connection config and stops cleanly through its control file', { timeout: 10000 }, async t => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'mailbeacon-fixture-cli-'));
  const configPath = path.join(dir, 'config.json'), reportPath = path.join(dir, 'report.json'), stopPath = path.join(dir, 'stop');
  const child = spawn(process.execPath, [path.resolve(__dirname, '../scripts/windows-mail-fixture.mjs'), '--config', configPath, '--report', reportPath, '--stop', stopPath], { stdio: ['ignore', 'pipe', 'pipe'] });
  let output = '';
  child.stdout.on('data', chunk => output += chunk);
  child.stderr.on('data', chunk => output += chunk);
  const exit = once(child, 'exit');
  t.after(async () => { if (child.exitCode === null) { child.kill(); await exit; } fs.rmSync(dir, { recursive: true, force: true }); });
  const deadline = Date.now() + 5000;
  while (!fs.existsSync(configPath) && Date.now() < deadline && child.exitCode === null) await new Promise(yes => setTimeout(yes, 10));
  assert.ok(fs.existsSync(configPath), 'fixture must publish ready config');
  const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));
  assert.equal(config.host, '127.0.0.1');
  assert.equal(config.pid, child.pid);
  assert.equal(config.reportPath, reportPath);
  await client(t, config);
  fs.writeFileSync(stopPath, 'stop');
  const [code] = await exit;
  assert.equal(code, 0);
  assert.equal(JSON.parse(fs.readFileSync(reportPath, 'utf8')).stopped, true);
  assert.ok(!output.includes(config.password));
  assert.ok(!output.includes(config.email));
});
