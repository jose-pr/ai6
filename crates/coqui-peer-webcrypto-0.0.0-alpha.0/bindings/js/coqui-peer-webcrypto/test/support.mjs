// Starting the Rust side, and what to do when a peer is absent: a skip that says so, or with
// COQUI_PEER_WEBCRYPTO_REQUIRE_PEERS set, as in CI, a failure.
import {spawn, spawnSync} from 'node:child_process';
import {createInterface} from 'node:readline';
import {fileURLToPath} from 'node:url';

const root = fileURLToPath(new URL('../../../../', import.meta.url));

export function unavailable(t, what) {
  if (process.env.COQUI_PEER_WEBCRYPTO_REQUIRE_PEERS) throw new Error(`${what} is required`);
  t.skip(`SKIPPED: ${what} is absent`);
}

export function haveCargo() {
  return spawnSync('cargo', ['--version'], {stdio: 'ignore'}).status === 0;
}

/** Builds once, so a test's timeout does not include compiling. */
export function buildPeer() {
  const built = spawnSync('cargo', ['build', '--quiet', '--example', 'peer_jsonl'], {cwd: root, stdio: 'inherit'});
  if (built.status !== 0) throw new Error('the Rust example does not build');
}

/** `peer_jsonl` running: `next()` is its next line, `send(line)` writes one, `exit` its status. */
export function startPeer(...args) {
  const child = spawn('cargo', ['run', '--quiet', '--example', 'peer_jsonl', '--', ...args], {
    cwd: root, stdio: ['pipe', 'pipe', 'pipe'],
  });
  const reader = createInterface({input: child.stdout})[Symbol.asyncIterator]();
  let errors = '';
  child.stderr.on('data', data => { errors += data.toString(); });
  const exit = new Promise(resolve => child.once('close', resolve));
  return {
    child, exit,
    errors: () => errors,
    send: line => child.stdin.write(line + '\n'),
    end: () => child.stdin.end(),
    async next() {
      const line = await reader.next();
      if (line.done) throw new Error('the Rust peer closed: ' + errors);
      return line.value;
    },
  };
}
