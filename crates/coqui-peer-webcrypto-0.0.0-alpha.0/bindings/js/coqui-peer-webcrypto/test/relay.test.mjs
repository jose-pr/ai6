// What a party carrying the connection can do, such as a reverse proxy that ends TLS. No TLS is
// involved anywhere here: the relay is the function that moves a message.
import test from 'node:test';
import assert from 'node:assert/strict';
import {createInitiator, createResponder, generateIdentity} from '../src/index.mjs';

async function parties() {
  const client = await generateIdentity();
  const server = await generateIdentity();
  return {
    client, server,
    i: await createInitiator(client, server.peerId),
    r: await createResponder(server, {allowClient: async id => id === client.peerId}),
  };
}

async function through(relay, p) {
  const challenge = await p.r.challenge(relay(p.i.hello()));
  const proof = await p.i.answer(relay(challenge));
  const accepted = await p.r.accept(relay(proof));
  const finished = await p.i.finish(relay(accepted.reply));
  return {atClient: finished.session, atServer: accepted.session, finished, accepted};
}

const reencode = (message, member, value) => {
  const parsed = JSON.parse(message);
  parsed[member] = value;
  return JSON.stringify(parsed);
};

test('a plain relay connects only the real parties, and reads and writes nothing after', async () => {
  const p = await parties();
  const seen = [];
  const {atClient, atServer, finished, accepted} = await through(message => {
    seen.push(message);
    return message;
  }, p);
  assert.equal(finished.peerId, p.server.peerId);
  assert.equal(accepted.peerId, p.client.peerId);
  assert.deepEqual(atClient.id, atServer.id);

  const record = await atClient.seal('secret from the relay');
  assert.ok(!Buffer.from(record).toString('latin1').includes('secret'));
  assert.equal(new TextDecoder().decode(await atServer.open(record)), 'secret from the relay');
  await assert.rejects(() => atServer.open(new TextEncoder().encode('{"method":"session/prompt"}')), /does not open/);
  await assert.rejects(async () => atServer.open(await atClient.seal('genuine')), /session is over/);
});

test('a relay cannot replay, reorder, reflect or alter a record', async () => {
  const replay = await through(m => m, await parties());
  const once = await replay.atClient.seal('one');
  await replay.atServer.open(once);
  await assert.rejects(() => replay.atServer.open(once), /does not open/);

  const reorder = await through(m => m, await parties());
  await reorder.atClient.seal('one');
  await assert.rejects(async () => reorder.atServer.open(await reorder.atClient.seal('two')), /does not open/);

  const reflect = await through(m => m, await parties());
  await assert.rejects(async () => reflect.atClient.open(await reflect.atClient.seal('one')), /does not open/);

  const alter = await through(m => m, await parties());
  const altered = await alter.atClient.seal('one');
  altered[0] ^= 1;
  await assert.rejects(() => alter.atServer.open(altered), /does not open/);

  const other = await through(m => m, await parties());
  const elsewhere = await through(m => m, await parties());
  await assert.rejects(async () => other.atServer.open(await elsewhere.atClient.seal('one')), /does not open/);
});

test('nothing after a bad record is believed, even at its right place', async () => {
  const {atClient, atServer} = await through(m => m, await parties());
  const first = await atClient.seal('one');
  const second = await atClient.seal('two');
  first[0] ^= 1;
  await assert.rejects(() => atServer.open(first), /does not open/);
  await assert.rejects(() => atServer.open(second), /session is over/);
});

test('a key swapped in the hello fails the proof the initiator checks', async () => {
  const p = await parties();
  const attacker = await generateIdentity();
  const r = await createResponder(p.server, {allowClient: async () => true});
  const swapped = reencode(p.i.hello(), 'publicKey', attacker.publicKey);
  await assert.rejects(async () => p.i.answer(await r.challenge(swapped)), /key proof failed/);
});

test('an ephemeral key or nonce swapped by a relay fails the proof', async () => {
  for (const member of ['ephemeral', 'nonce']) {
    const other = JSON.parse((await parties()).i.hello());
    const p = await parties();
    const swapped = reencode(p.i.hello(), member, other[member]);
    await assert.rejects(async () => p.i.answer(await p.r.challenge(swapped)), /key proof failed/, member);

    const q = await parties();
    const challenge = await q.r.challenge(q.i.hello());
    await assert.rejects(() => q.i.answer(reencode(challenge, member, other[member])), /key proof failed/, member);
  }
});

test('a relay answering with a key of its own fails the pin', async () => {
  const p = await parties();
  const relay = await generateIdentity();
  const r = await createResponder(relay, {allowClient: async () => true});
  await assert.rejects(async () => p.i.answer(await r.challenge(p.i.hello())), /not pinned/);
});

test('an outer binding is optional, and two different channels are refused', async () => {
  const client = await generateIdentity();
  const server = await generateIdentity();
  const pair = async (a, b) => ({
    i: await createInitiator(client, server.peerId, {binding: a}),
    r: await createResponder(server, {allowClient: async () => true, binding: b}),
  });
  const same = await pair(new Uint8Array([1]), new Uint8Array([1]));
  assert.equal(typeof await same.i.answer(await same.r.challenge(same.i.hello())), 'string');
  const two = await pair(new Uint8Array([1]), new Uint8Array([2]));
  await assert.rejects(async () => two.i.answer(await two.r.challenge(two.i.hello())), /key proof failed/);
  const one = await pair(new Uint8Array([1]), new Uint8Array());
  await assert.rejects(async () => one.i.answer(await one.r.challenge(one.i.hello())), /key proof failed/);
});

test('a relay cutting at each record boundary is reported as truncation', async () => {
  const count = 4;
  for (let delivered = 0; delivered <= count; delivered += 1) {
    const {atClient, atServer} = await through(m => m, await parties());
    const records = [];
    for (let index = 0; index < count; index += 1) records.push(await atClient.seal(String(index)));
    records.push(await atClient.close());
    for (const record of records.slice(0, delivered)) await atServer.open(record);
    assert.throws(() => atServer.verifyEnd(), /truncated/, `cut after ${delivered} records`);
  }
});

test('a stream ending with its close record is complete, and each direction closes on its own', async () => {
  const {atClient, atServer} = await through(m => m, await parties());
  assert.throws(() => atServer.verifyEnd(), /truncated/);
  const close = await atClient.close();
  await assert.rejects(() => atClient.seal('late'), /already closed/);
  await assert.rejects(() => atClient.close(), /already closed/);
  assert.equal(await atServer.open(close), null);
  atServer.verifyEnd();
  await assert.rejects(() => atServer.open(close), /session is over|does not open/);
  const other = await through(m => m, await parties());
  assert.equal(await other.atServer.open(await other.atClient.close()), null);
  const reply = await other.atServer.seal('last words');
  assert.equal(new TextDecoder().decode(await other.atClient.open(reply)), 'last words');
});

test('a failed open ends sending too', async () => {
  const {atServer} = await through(m => m, await parties());
  await assert.rejects(() => atServer.open(new Uint8Array(17)), /does not open/);
  await assert.rejects(() => atServer.seal('after'), /session is over/);
  await assert.rejects(() => atServer.close(), /session is over/);
  assert.throws(() => atServer.verifyEnd(), /session is over/);
});

test('seal and open read their input when called', async () => {
  const {atClient, atServer} = await through(m => m, await parties());
  const message = new TextEncoder().encode('original');
  const pending = atClient.seal(message);
  message.fill(88);
  const record = await pending;
  const opening = atServer.open(record);
  record.fill(0);
  assert.equal(new TextDecoder().decode(await opening), 'original');
});

test('a destroyed session does nothing more', async () => {
  const {atClient, atServer} = await through(m => m, await parties());
  const record = await atClient.seal('before');
  atServer.destroy();
  await assert.rejects(() => atServer.open(record), /session is over/);
  await assert.rejects(() => atServer.seal('after'), /session is over/);
  await assert.rejects(() => atServer.export('label', 32), /session is over/);
  assert.throws(() => atServer.verifyEnd(), /session is over/);
});

test('the session limit stops a direction, and a close still passes', async () => {
  const client = await generateIdentity();
  const server = await generateIdentity();
  const limit = 2 * (1 + 10 + 16 + 32);
  const i = await createInitiator(client, server.peerId, {sessionLimit: limit});
  const r = await createResponder(server, {allowClient: async () => true, sessionLimit: limit});
  const {atClient, atServer} = await through(m => m, {i, r});
  for (let round = 0; round < 2; round += 1) {
    await atServer.open(await atClient.seal(new Uint8Array(10)));
  }
  await assert.rejects(() => atClient.seal(new Uint8Array(10)), /session limit/);
  assert.equal(await atServer.open(await atClient.close()), null);
  atServer.verifyEnd();
});

test('a limit of zero is allowed, and a peer past it is refused at the opener', async () => {
  const client = await generateIdentity();
  const server = await generateIdentity();
  const i = await createInitiator(client, server.peerId);
  const r = await createResponder(server, {allowClient: async () => true, sessionLimit: 0});
  const {atClient, atServer} = await through(m => m, {i, r});
  await assert.rejects(async () => atServer.open(await atClient.seal('x')), /session limit/);
  await assert.rejects(() => atServer.seal('x'), /session limit/);
});

test('exports of one label and two lengths are unrelated', async () => {
  const {atClient, atServer} = await through(m => m, await parties());
  const short = await atClient.export('label', 16);
  const long = await atClient.export('label', 32);
  assert.notDeepEqual(short, long.subarray(0, 16));
  assert.deepEqual(await atServer.export('label', 16), short);
});

test('a binding must be bytes, not something that turns into other bytes or none', async () => {
  const client = await generateIdentity();
  const server = await generateIdentity();
  const bytes = new Uint8Array([1, 2, 3]);
  for (const binding of [bytes.buffer, new DataView(bytes.buffer), 'abc', [1, 2, 3], 7, null]) {
    await assert.rejects(() => createInitiator(client, server.peerId, {binding}), /binding must be/);
    await assert.rejects(() => createResponder(server, {allowClient: async () => true, binding}), /binding must be/);
  }
  await createInitiator(client, server.peerId, {binding: Buffer.from(bytes)});
  await createInitiator(client, server.peerId, {binding: bytes});
});
