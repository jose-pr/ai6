// The vectors the Rust tests pin: this implementation must reach the same bytes and refuse the same
// messages. WebCrypto's ECDSA is randomised, so its own signatures are checked rather than compared.
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {webcrypto} from 'node:crypto';
import {createInitiator, createResponder, importIdentity} from '../src/index.mjs';

const vectors = JSON.parse(readFileSync(new URL('../../../../tests/vectors/handshake.json', import.meta.url)));
const hex = text => Uint8Array.from(Buffer.from(text, 'hex'));
const b64u = bytes => Buffer.from(bytes).toString('base64url');

async function ephemeralOf(side) {
  const point = hex(side.ephemeralPoint);
  const jwk = {
    kty: 'EC', crv: 'P-256', ext: true,
    x: b64u(point.subarray(1, 33)), y: b64u(point.subarray(33)), d: b64u(hex(side.ephemeralScalar)),
  };
  const algorithm = {name: 'ECDH', namedCurve: 'P-256'};
  const {d: _d, ...publicJwk} = jwk;
  return {
    privateKey: await webcrypto.subtle.importKey('jwk', jwk, algorithm, false, ['deriveBits']),
    publicKey: await webcrypto.subtle.importKey('jwk', publicJwk, algorithm, true, []),
  };
}

function scripted(side) {
  return {
    random: bytes => bytes.set(hex(side.nonce)),
    ephemeral: () => ephemeralOf(side),
  };
}

const identity = side => importIdentity(hex(side.pkcs8));

async function initiator() {
  return createInitiator(await identity(vectors.initiator), vectors.responder.peerId, scripted(vectors.initiator));
}

async function responder() {
  return createResponder(await identity(vectors.responder), {
    allowClient: async id => id === vectors.initiator.peerId,
    ...scripted(vectors.responder),
  });
}

const verifies = async (side, role, signature) => webcrypto.subtle.verify(
  {name: 'ECDSA', hash: 'SHA-256'},
  await webcrypto.subtle.importKey('spki', hex(side.spki), {name: 'ECDSA', namedCurve: 'P-256'}, false, ['verify']),
  Buffer.from(signature, 'base64url'),
  hex(vectors.signed[role]),
);

test('identities give the vector ids and keys', async () => {
  for (const side of [vectors.initiator, vectors.responder]) {
    const imported = await identity(side);
    assert.equal(imported.peerId, side.peerId);
    assert.deepEqual(Buffer.from(imported.publicKey, 'base64url'), Buffer.from(hex(side.spki)));
  }
});

test('an initiator reaches the vector session from the vector messages', async () => {
  const i = await initiator();
  assert.equal(i.hello(), vectors.messages.hello);
  const proof = await i.answer(vectors.messages.challenge);
  assert.ok(await verifies(vectors.initiator, 'client', JSON.parse(proof).signature));
  const {peerId, session} = await i.finish(vectors.messages.accept);
  assert.equal(peerId, vectors.responder.peerId);
  assert.equal(Buffer.from(session.id).toString('hex'), vectors.session.id);
  assert.equal(Buffer.from(await session.export('EXPORTER-coqui-pair', 32)).toString('hex'), vectors.session.pairExport);
  for (const {from, message, record} of vectors.records) {
    if (from === 'initiator') {
      assert.equal(Buffer.from(await session.seal(message)).toString('hex'), record);
    } else {
      assert.equal(new TextDecoder().decode(await session.open(hex(record))), message);
    }
  }
  assert.equal(Buffer.from(await session.close()).toString('hex'), vectors.closeRecord);
  await assert.rejects(() => session.seal('after'), /already closed/);
});

test('a responder reaches the vector session from the vector messages', async () => {
  const r = await responder();
  const challenge = await r.challenge(vectors.messages.hello);
  const {signature, ...rest} = JSON.parse(challenge);
  const {signature: _expected, ...expected} = JSON.parse(vectors.messages.challenge);
  assert.deepEqual(rest, expected);
  assert.ok(await verifies(vectors.responder, 'server', signature));
  const {peerId, session, reply} = await r.accept(vectors.messages.proof);
  assert.equal(peerId, vectors.initiator.peerId);
  assert.ok(await verifies(vectors.responder, 'accept', JSON.parse(reply).signature));
  assert.equal(Buffer.from(session.id).toString('hex'), vectors.session.id);
  for (const {from, message, record} of vectors.records) {
    if (from === 'responder') {
      assert.equal(Buffer.from(await session.seal(message)).toString('hex'), record);
    } else {
      assert.equal(new TextDecoder().decode(await session.open(hex(record))), message);
    }
  }
  assert.equal(await session.open(hex(vectors.closeRecord)), null);
  session.verifyEnd();
});

for (const {name, record} of vectors.refusedRecords) {
  test(`refused record: ${name}`, async () => {
    const i = await initiator();
    i.hello();
    await i.answer(vectors.messages.challenge);
    const {session} = await i.finish(vectors.messages.accept);
    await session.open(hex(vectors.records[2].record));
    await assert.rejects(() => session.open(hex(record)), /does not open/);
    await assert.rejects(() => session.seal('after'), /session is over/);
  });
}

test('a signature with either s is accepted', async () => {
  const i = await initiator();
  i.hello();
  assert.equal(typeof await i.answer(vectors.challengeOtherS), 'string');
});

for (const {stage, name, message} of vectors.refused) {
  test(`refused at ${stage}: ${name}`, async () => {
    if (stage === 'hello') {
      const r = await responder();
      await assert.rejects(() => r.challenge(message), /Invalid|Oversized|proof/);
      await assert.rejects(() => r.challenge(vectors.messages.hello), /Unexpected/);
      // The same bytes arriving as bytes, where a decoder could drop a byte order mark.
      const asBytes = await responder();
      await assert.rejects(() => asBytes.challenge(new TextEncoder().encode(message)), /Invalid|Oversized|proof/);
    } else if (stage === 'challenge') {
      const i = await initiator();
      i.hello();
      await assert.rejects(() => i.answer(message));
      await assert.rejects(() => i.answer(vectors.messages.challenge), /Unexpected/);
    } else if (stage === 'proof') {
      const r = await responder();
      await r.challenge(vectors.messages.hello);
      await assert.rejects(() => r.accept(message));
      await assert.rejects(() => r.accept(vectors.messages.proof), /Unexpected/);
    } else {
      const i = await initiator();
      i.hello();
      await i.answer(vectors.messages.challenge);
      await assert.rejects(() => i.finish(message));
      await assert.rejects(() => i.finish(vectors.messages.accept), /Unexpected/);
    }
  });
}
