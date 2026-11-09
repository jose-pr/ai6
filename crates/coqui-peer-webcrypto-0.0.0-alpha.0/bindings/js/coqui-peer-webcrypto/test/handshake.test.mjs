import test from 'node:test';
import assert from 'node:assert/strict';
import {webcrypto} from 'node:crypto';
import {setFlagsFromString} from 'node:v8';
import {runInNewContext} from 'node:vm';
import {
  DEFAULT_MAX_MESSAGE, METHOD, PROFILE, createInitiator, createResponder, generateIdentity,
  importIdentity, loadOrCreateIdentity, profile,
} from '../src/index.mjs';

async function setup(allow = true) {
  const client = await generateIdentity();
  const server = await generateIdentity();
  return {
    client, server,
    initiator: await createInitiator(client, server.peerId),
    responder: await createResponder(server, {allowClient: async id => allow && id === client.peerId}),
  };
}

async function complete(pair) {
  const challenge = await pair.responder.challenge(pair.initiator.hello());
  const proof = await pair.initiator.answer(challenge);
  const accepted = await pair.responder.accept(proof);
  const finished = await pair.initiator.finish(accepted.reply);
  return {accepted, finished, proof};
}

test('identities cannot be extracted and carry coqui-peer ids', async () => {
  const first = await generateIdentity();
  const second = await generateIdentity();
  assert.match(first.peerId, /^sha256:[a-f0-9]{64}$/);
  assert.notEqual(first.peerId, second.peerId);
  assert.equal(first.privateKey.extractable, false);
  assert.equal(first.method, 'webcrypto');
  assert.deepEqual({...profile}, {id: PROFILE, version: 1, method: METHOD});
  await assert.rejects(() => webcrypto.subtle.exportKey('pkcs8', first.privateKey));
});

test('an extractable key, or an id that is not its own, is refused', async () => {
  const good = await generateIdentity();
  const pair = await webcrypto.subtle.generateKey({name: 'ECDSA', namedCurve: 'P-256'}, true, ['sign', 'verify']);
  const extractable = {...good, privateKey: pair.privateKey};
  await assert.rejects(() => createInitiator(extractable, good.peerId), /non-extractable/);
  const wrongId = {...good, peerId: 'sha256:' + '0'.repeat(64)};
  await assert.rejects(() => createResponder(wrongId, {allowClient: async () => true}), /non-extractable/);
});

test('a PKCS#8 key imports with the id its public key hashes to', async () => {
  const pair = await webcrypto.subtle.generateKey({name: 'ECDSA', namedCurve: 'P-256'}, true, ['sign', 'verify']);
  const identity = await importIdentity(new Uint8Array(await webcrypto.subtle.exportKey('pkcs8', pair.privateKey)));
  assert.equal(identity.privateKey.extractable, false);
  const p = await createInitiator(identity, identity.peerId);
  assert.equal(typeof p.hello(), 'string');
});

test('both sides prove their keys and share one session', async () => {
  const p = await setup();
  const {accepted, finished} = await complete(p);
  assert.equal(accepted.peerId, p.client.peerId);
  assert.equal(finished.peerId, p.server.peerId);
  assert.deepEqual(finished.session.id, accepted.session.id);
  const record = await finished.session.seal('hello');
  assert.equal(new TextDecoder().decode(await accepted.session.open(record)), 'hello');
  const back = await accepted.session.seal(new Uint8Array([1, 2, 3]));
  assert.deepEqual(await finished.session.open(back), new Uint8Array([1, 2, 3]));
  assert.deepEqual(await finished.session.export('x', 32), await accepted.session.export('x', 32));
  assert.notDeepEqual(await finished.session.export('x', 32), await finished.session.export('y', 32));
});

test('records keep the order of the calls that made them', async () => {
  const p = await setup();
  const {accepted, finished} = await complete(p);
  const records = await Promise.all([1, 2, 3, 4].map(n => finished.session.seal(String(n))));
  const opened = await Promise.all(records.map(record => accepted.session.open(record)));
  assert.deepEqual(opened.map(bytes => new TextDecoder().decode(bytes)), ['1', '2', '3', '4']);
});

test('an unknown client is refused after its proof and gets no session', async () => {
  const p = await setup(false);
  const response = await p.responder.challenge(p.initiator.hello());
  const proof = await p.initiator.answer(response);
  await assert.rejects(() => p.responder.accept(proof), /not paired/);
  await assert.rejects(() => p.responder.accept(proof), /Unexpected/);
});

test('allowClient admits only on exactly true', async () => {
  for (const answer of ['yes', 1, {}, undefined]) {
    const client = await generateIdentity();
    const server = await generateIdentity();
    const i = await createInitiator(client, server.peerId);
    const r = await createResponder(server, {allowClient: async () => answer});
    const proof = await i.answer(await r.challenge(i.hello()));
    await assert.rejects(() => r.accept(proof), /not paired/);
  }
});

test('an unpinned responder is refused before the client signs anything', async () => {
  const p = await setup();
  const bad = await createInitiator(p.client, (await generateIdentity()).peerId);
  const response = await p.responder.challenge(bad.hello());
  await assert.rejects(() => bad.answer(response), /not pinned/);
});

test('a proof from another connection does not validate', async () => {
  const p = await setup();
  const proof = await p.initiator.answer(await p.responder.challenge(p.initiator.hello()));
  const again = await createResponder(p.server, {allowClient: async id => id === p.client.peerId});
  const newClient = await createInitiator(p.client, p.server.peerId);
  await again.challenge(newClient.hello());
  await assert.rejects(() => again.accept(proof), /key proof failed/);
});

test('a self-connection is refused', async () => {
  const identity = await generateIdentity();
  const i = await createInitiator(identity, identity.peerId);
  const r = await createResponder(identity, {allowClient: async () => true});
  await assert.rejects(() => r.challenge(i.hello()), /Invalid/);
});

test('messages out of order are refused and a failure is final', async () => {
  const p = await setup();
  await assert.rejects(() => p.initiator.finish('{}'), /Unexpected/);
  const hello = p.initiator.hello();
  assert.throws(() => p.initiator.hello(), /Unexpected/);
  await assert.rejects(() => p.responder.accept(hello), /Unexpected/);
  await assert.rejects(() => p.responder.challenge('not json'), /Unexpected|Invalid/);
  const q = await setup();
  q.initiator.hello();
  await assert.rejects(() => q.responder.challenge('not json'), /Invalid/);
  await assert.rejects(() => q.responder.challenge(hello), /Unexpected/);
});

test('the message limit is a setting with a default and no floor', async () => {
  const client = await generateIdentity();
  const server = await generateIdentity();
  const r = await createResponder(server, {allowClient: async () => true});
  await assert.rejects(() => r.challenge('x'.repeat(DEFAULT_MAX_MESSAGE + 1)), /Oversized/);
  const tiny = await createResponder(server, {allowClient: async () => true, maxMessage: 16});
  const i = await createInitiator(client, server.peerId);
  await assert.rejects(() => tiny.challenge(i.hello()), /Oversized/);
  const large = await createResponder(server, {allowClient: async () => true, maxMessage: 1 << 20});
  const j = await createInitiator(client, server.peerId);
  assert.equal(typeof await large.challenge(j.hello()), 'string');
});

test('a message may arrive as bytes as well as text', async () => {
  const p = await setup();
  const challenge = await p.responder.challenge(new TextEncoder().encode(p.initiator.hello()));
  assert.equal(typeof await p.initiator.answer(new TextEncoder().encode(challenge)), 'string');
});

test('errors quote nothing the peer sent', async () => {
  const p = await setup();
  p.initiator.hello();
  await assert.rejects(() => p.responder.challenge('attacker text'), error => !error.message.includes('attacker'));
});

test('an identity store needs a name, and IndexedDB', async () => {
  await assert.rejects(() => loadOrCreateIdentity(), /store name/);
  await assert.rejects(() => loadOrCreateIdentity('name'), /IndexedDB/);
});

test('the ephemeral private key is unreachable once the handshake is done', async () => {
  setFlagsFromString('--expose-gc');
  const collect = runInNewContext('gc');
  const client = await generateIdentity();
  const server = await generateIdentity();
  const refs = {};
  const tracked = name => async () => {
    const pair = await webcrypto.subtle.generateKey({name: 'ECDH', namedCurve: 'P-256'}, false, ['deriveBits']);
    refs[name] = new WeakRef(pair.privateKey);
    return pair;
  };
  const i = await createInitiator(client, server.peerId, {ephemeral: tracked('initiator')});
  const r = await createResponder(server, {allowClient: async () => true, ephemeral: tracked('responder')});
  const accepted = await r.accept(await i.answer(await r.challenge(i.hello())));
  const finished = await i.finish(accepted.reply);
  for (let round = 0; round < 10 && (refs.initiator.deref() || refs.responder.deref()); round += 1) {
    await new Promise(resolve => setImmediate(resolve));
    collect();
  }
  assert.equal(refs.initiator.deref(), undefined);
  assert.equal(refs.responder.deref(), undefined);
  // The handshake objects and sessions are still alive here.
  assert.ok(i && r && finished.session && accepted.session);
});
