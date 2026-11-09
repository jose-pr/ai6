// A real browser, where Node's WebCrypto stands in nowhere: the module in a page on a secure origin,
// its key kept in IndexedDB, against the Rust responder. Point COQUI_TEST_BROWSER at Chrome or Edge
// when it is not in a usual place.
import test, {before} from 'node:test';
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {existsSync, mkdtempSync, readFileSync} from 'node:fs';
import {rm} from 'node:fs/promises';
import {createServer} from 'node:http';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {buildPeer, haveCargo, startPeer, unavailable} from './support.mjs';

const candidates = [
  process.env.COQUI_TEST_BROWSER,
  '/usr/bin/google-chrome', '/usr/bin/chromium', '/usr/bin/chromium-browser',
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  'C:/Program Files/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
];
const browser = candidates.find(path => path && existsSync(path));
const cargo = haveCargo();

before(() => { if (cargo && browser) buildPeer(); }, {timeout: 600_000});

const page = `<!doctype html><meta charset="utf-8"><title>test</title><script type="module">
import {generateIdentity, loadOrCreateIdentity, createInitiator} from '/index.mjs';
const post = async (path, body) => (await fetch(path, {method: 'POST', body})).text();
const hex = bytes => [...bytes].map(byte => byte.toString(16).padStart(2, '0')).join('');
const unhex = text => Uint8Array.from(text.match(/../g), pair => parseInt(pair, 16));
try {
  const report = {secure: isSecureContext};
  const fresh = await generateIdentity();
  report.extractable = fresh.privateKey.extractable;
  const name = 'coqui-test-' + hex(crypto.getRandomValues(new Uint8Array(8)));
  const first = await loadOrCreateIdentity(name);
  const second = await loadOrCreateIdentity(name);
  report.sameIdentity = first.peerId === second.peerId;
  report.storedExtractable = second.privateKey.extractable;
  let exported = false;
  try { await crypto.subtle.exportKey('pkcs8', second.privateKey); exported = true; } catch {}
  report.exported = exported;
  const peerId = await post('/start', second.peerId);
  const initiator = await createInitiator(second, peerId);
  const challenge = await post('/send', initiator.hello());
  const accept = await post('/send', await initiator.answer(challenge));
  const {session} = await initiator.finish(accept);
  report.id = hex(session.id);
  report.peerId = peerId;
  report.echo = new TextDecoder().decode(
    await session.open(unhex(await post('/send', hex(await session.seal('ping from a browser'))))),
  );
  const closing = await post('/send', hex(await session.close()));
  report.closed = (await session.open(unhex(closing))) === null;
  session.verifyEnd();
  session.destroy();
  report.ok = true;
  await post('/result', JSON.stringify(report));
} catch (error) {
  await post('/result', JSON.stringify({ok: false, error: String(error)}));
}
</script>`;

test('a browser keeps its key, authenticates a Rust responder and exchanges records', {timeout: 120_000}, async t => {
  if (!cargo) return unavailable(t, 'cargo');
  if (!browser) return unavailable(t, 'a browser');
  const module = readFileSync(new URL('../src/index.mjs', import.meta.url));
  let peer;
  let sessionId;
  let settle;
  const finished = new Promise(resolve => { settle = resolve; });
  const server = createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const body = Buffer.concat(chunks).toString();
    const answer = (type, text) => {
      response.writeHead(200, {'content-type': type});
      response.end(text);
    };
    if (request.url === '/') return answer('text/html', page);
    if (request.url === '/index.mjs') return answer('text/javascript', module);
    if (request.url === '/start') {
      peer = startPeer('responder', body);
      return answer('text/plain', JSON.parse(await peer.next()).peerId);
    }
    if (request.url === '/send') {
      peer.send(body);
      const line = await peer.next();
      if (line.includes('accept')) sessionId = await peer.next();
      return answer('text/plain', line);
    }
    if (request.url === '/result') {
      settle(JSON.parse(body));
      return answer('text/plain', '');
    }
    response.writeHead(404);
    response.end();
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const profile = mkdtempSync(join(tmpdir(), 'coqui-browser-'));
  const running = spawn(browser, [
    '--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check',
    // A CI runner's kernel may forbid the sandbox's namespaces; this page is the test's own.
    ...(process.env.CI ? ['--no-sandbox'] : []),
    `--user-data-dir=${profile}`, `http://127.0.0.1:${server.address().port}/`,
  ], {stdio: 'ignore'});
  const closed = new Promise(resolve => running.once('close', resolve));
  try {
    // Node's own test timeout would leave the server and browser holding the process open.
    const report = await Promise.race([
      finished,
      closed.then(() => ({ok: false, error: 'the browser exited with no report'})),
      new Promise(resolve => {
        setTimeout(resolve, 100_000, {ok: false, error: 'the page sent no report'}).unref();
      }),
    ]);
    assert.equal(report.ok, true, report.error);
    assert.equal(report.secure, true);
    assert.equal(report.extractable, false);
    assert.equal(report.storedExtractable, false);
    assert.equal(report.exported, false);
    assert.equal(report.sameIdentity, true);
    assert.equal(report.echo, 'echo:ping from a browser');
    assert.equal(report.id, sessionId);
    assert.equal(report.closed, true);
    peer.end();
    assert.equal(await peer.exit, 0);
  } finally {
    peer?.child.kill();
    running.kill();
    await closed;
    server.close();
    server.closeAllConnections();
    // Chrome's helpers can outlive its main process and still write here, and a directory left in
    // the temporary folder is no reason to fail a test whose assertions passed.
    await rm(profile, {recursive: true, force: true, maxRetries: 10, retryDelay: 200})
      .catch(error => t.diagnostic(`left ${profile} behind: ${error.code}`));
  }
});
