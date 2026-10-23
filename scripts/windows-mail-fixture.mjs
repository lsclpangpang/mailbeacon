// Isolated IMAP endpoint for installed-app checks. It cannot contact a real
// mailbox and never records LOGIN arguments. Trust its CA only in the test
// application's MAILPULSE_EXTRA_CA environment, never in the system store.
import tls from 'node:tls';
import { randomBytes } from 'node:crypto';
import { readFileSync, writeFileSync, renameSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const certificateDirectory = new URL('../tests/fixtures/windows-mail-tls/', import.meta.url);
const capabilities = 'IMAP4rev1 IDLE ID';

function loginWords(text) {
  const parts = text.match(/"(?:[^"\\]|\\.)*"|[^\s]+/g) || [];
  if (parts.length !== 2 || parts.some(word => /[{}\r\n]/.test(word))) return null;
  return parts.map(word => word.startsWith('"') ? word.slice(1, -1).replace(/\\(.)/g, '$1') : word);
}

export async function startMailFixture({ reportPath }) {
  reportPath = resolve(reportPath);
  if (existsSync(reportPath)) throw new Error('Fixture report must be a new file.');
  const caPath = fileURLToPath(new URL('ca.pem', certificateDirectory));
  const account = { email: 'windows-smoke@example.invalid', name: 'Windows Smoke',
    password: randomBytes(24).toString('hex'), invalidPassword: 'invalid-test-password' };
  const state = { version: 1, loopbackOnly: true, connections: 0, tlsErrors: 0,
    authRejected: 0, authAccepted: 0, selectAccepted: 0, commands: {}, stages: ['starting'], stopped: false };
  const record = (command, stage) => {
    if (command) state.commands[command] = (state.commands[command] || 0) + 1;
    if (stage && !state.stages.includes(stage)) state.stages.push(stage);
    writeFileSync(reportPath + '.tmp', JSON.stringify(state, null, 2) + '\n', { mode: 0o600 });
    renameSync(reportPath + '.tmp', reportPath);
  };
  const connections = new Set();
  const server = tls.createServer({
    key: readFileSync(new URL('server-key.pem', certificateDirectory)),
    cert: readFileSync(new URL('server.pem', certificateDirectory)),
    minVersion: 'TLSv1.2',
  }, socket => {
    state.connections++;
    record(null, 'connected');
    let buffer = '', authenticated = false, idleTag = null, commandCount = 0;
    socket.setEncoding('utf8');
    socket.setTimeout(120000, () => socket.destroy());
    socket.on('error', () => {});
    const write = line => socket.write(line + '\r\n');
    write(`* OK [CAPABILITY ${capabilities}] MailBeacon isolated fixture`);
    socket.on('data', chunk => {
      buffer += chunk;
      if (buffer.length > 16384) { record('OVERSIZE'); socket.destroy(); return; }
      while (buffer.includes('\r\n')) {
        const end = buffer.indexOf('\r\n'), line = buffer.slice(0, end);
        buffer = buffer.slice(end + 2);
        if (++commandCount > 1000) { record('COMMAND_LIMIT'); socket.destroy(); return; }
        if (idleTag) {
          if (line.toUpperCase() !== 'DONE') { socket.destroy(); return; }
          record('DONE'); write(`${idleTag} OK IDLE completed`); idleTag = null; continue;
        }
        const parsed = /^([A-Za-z0-9._-]+) +([A-Za-z]+)(?: +(.*))?$/.exec(line);
        if (!parsed) { record('MALFORMED'); write('* BAD malformed-command'); continue; }
        const [, tag, verb, raw = ''] = parsed, command = verb.toUpperCase();
        // Only names from this allowlist enter the report; no user-supplied
        // argument, malformed tag, password or full command is persisted.
        const known = ['LOGIN','CAPABILITY','ID','NOOP','LIST','LSUB','STATUS','SELECT','EXAMINE','SEARCH','FETCH','UID','IDLE','LOGOUT','CLOSE','CHECK'];
        record(known.includes(command) ? command : 'UNSUPPORTED');
        if (command === 'LOGOUT') { write('* BYE fixture logout'); socket.end(`${tag} OK LOGOUT completed\r\n`); continue; }
        if (command === 'CAPABILITY') { write(`* CAPABILITY ${capabilities}`); write(`${tag} OK CAPABILITY completed`); continue; }
        if (command === 'LOGIN') {
          const words = loginWords(raw);
          if (!words) { write(`${tag} BAD malformed-login`); continue; }
          if (words[0] !== account.email || words[1] !== account.password) {
            state.authRejected++; authenticated = false; record(null, 'authentication-rejected');
            write(`${tag} NO [AUTHENTICATIONFAILED] fixture-auth-rejected`);
          } else {
            state.authAccepted++; authenticated = true; record(null, 'authenticated');
            write(`${tag} OK LOGIN completed`);
          }
          continue;
        }
        if (!authenticated) { write(`${tag} NO [AUTHENTICATIONREQUIRED] login-required`); continue; }
        switch (command) {
          case 'ID': write('* ID ("name" "MailBeacon fixture")'); break;
          case 'LIST': case 'LSUB': write(`* ${command} (\\HasNoChildren) "/" "INBOX"`); break;
          case 'SELECT': case 'EXAMINE':
            if (!/^"?INBOX"?$/i.test(raw)) { write(`${tag} NO unknown-mailbox`); continue; }
            write('* FLAGS (\\Seen \\Answered \\Flagged \\Deleted \\Draft)');
            write('* 0 EXISTS'); write('* 0 RECENT');
            write('* OK [UIDVALIDITY 1] valid'); write('* OK [UIDNEXT 1] next');
            write('* OK [PERMANENTFLAGS (\\Seen \\Answered \\Flagged \\Deleted \\Draft)] flags');
            if (command === 'SELECT') state.selectAccepted++;
            record(null, 'selected');
            write(`${tag} OK [READ-WRITE] ${command} completed`); continue;
          case 'STATUS': write('* STATUS "INBOX" (MESSAGES 0 UNSEEN 0 UIDNEXT 1 UIDVALIDITY 1)'); break;
          case 'SEARCH': write('* SEARCH'); break;
          case 'FETCH': break;
          case 'UID': {
            const sub = raw.split(/\s+/, 1)[0].toUpperCase();
            if (!['SEARCH', 'FETCH'].includes(sub)) { write(`${tag} BAD unsupported-command`); continue; }
            record(`UID ${sub}`);
            if (sub === 'SEARCH') write('* SEARCH');
            write(`${tag} OK ${sub} completed`); continue;
          }
          case 'IDLE': idleTag = tag; record(null, 'idling'); write('+ idling'); continue;
          case 'NOOP': case 'CLOSE': case 'CHECK': break;
          default: write(`${tag} BAD unsupported-command`); continue;
        }
        write(`${tag} OK ${command} completed`);
      }
    });
  });
  server.on('connection', socket => {
    connections.add(socket);
    socket.on('close', () => connections.delete(socket));
    if (connections.size > 32) socket.destroy();
  });
  server.on('tlsClientError', () => { state.tlsErrors++; record(null, 'tls-error'); });
  await new Promise((yes, no) => { server.once('error', no); server.listen(0, '127.0.0.1', yes); });
  const config = { version: 1, host: '127.0.0.1', imapPort: server.address().port,
    smtpHost: '127.0.0.1', smtpPort: 9, ...account, caPath, reportPath, pid: process.pid };
  record(null, 'ready');
  let closing;
  return { config, report: () => structuredClone(state), close() {
    return closing ||= new Promise(resolveClose => {
      for (const socket of connections) socket.destroy();
      server.close(() => { state.stopped = true; record(null, 'stopped'); resolveClose(); });
    });
  } };
}

async function main() {
  const args = process.argv.slice(2), options = {};
  while (args.length) {
    const key = args.shift(), value = args.shift();
    if (!['--config', '--report', '--stop'].includes(key) || !value || options[key]) throw new Error('Expected unique --config, --report and --stop file paths.');
    options[key] = resolve(value);
  }
  if (Object.keys(options).length !== 3 || new Set(Object.values(options)).size !== 3) throw new Error('Fixture requires three distinct output/control paths.');
  if (existsSync(options['--config']) || existsSync(options['--stop'])) throw new Error('Fixture paths must be new.');
  const fixture = await startMailFixture({ reportPath: options['--report'] });
  writeFileSync(options['--config'] + '.tmp', JSON.stringify(fixture.config, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
  renameSync(options['--config'] + '.tmp', options['--config']);
  console.log('Loopback mail fixture ready.');
  let closing = false;
  const finish = async () => { if (closing) return; closing = true; clearInterval(timer); await fixture.close(); };
  const timer = setInterval(() => { if (existsSync(options['--stop'])) void finish(); }, 100);
  process.once('SIGTERM', finish); process.once('SIGINT', finish);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
}
