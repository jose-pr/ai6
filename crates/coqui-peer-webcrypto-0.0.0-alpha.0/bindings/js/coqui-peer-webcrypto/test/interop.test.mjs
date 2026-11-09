// JavaScript against the Rust implementation, each in both roles, over real pipes.
import test, {before} from 'node:test';
import assert from 'node:assert/strict';
import {createInitiator, createResponder, generateIdentity} from '../src/index.mjs';
import {buildPeer, haveCargo, startPeer, unavailable} from './support.mjs';

const toHex = bytes => Buffer.from(bytes).toString('hex');
const fromHex = text => Uint8Array.from(Buffer.from(text, 'hex'));
const cargo = haveCargo();

before(() => { if (cargo) buildPeer(); }, {timeout: 600_000});

test('a JavaScript initiator authenticates a Rust responder and they exchange records', async t => {
  if (!cargo) return unavailable(t, 'cargo');
  const client = await generateIdentity();
  const peer = startPeer('responder', client.peerId);
  try {
    const {peerId} = JSON.parse(await peer.next());
    const initiator = await createInitiator(client, peerId);
    peer.send(initiator.hello());
    peer.send(await initiator.answer(await peer.next()));
    const {session, peerId: proven} = await initiator.finish(await peer.next());
    assert.equal(proven, peerId);
    assert.equal(await peer.next(), toHex(session.id));
    peer.send(toHex(await session.seal('ping from javascript')));
    const reply = await session.open(fromHex(await peer.next()));
    assert.equal(new TextDecoder().decode(reply), 'echo:ping from javascript');
    peer.send(toHex(await session.close()));
    assert.equal(await session.open(fromHex(await peer.next())), null);
    session.verifyEnd();
    peer.end();
    assert.equal(await peer.exit, 0, peer.errors());
    assert.match(peer.errors(), new RegExp(`verified ${client.peerId}`));
  } finally {
    peer.child.kill();
  }
});

test('a Rust responder refuses a JavaScript peer it does not know', async t => {
  if (!cargo) return unavailable(t, 'cargo');
  const client = await generateIdentity();
  const peer = startPeer('responder', 'sha256:' + '0'.repeat(64));
  try {
    const {peerId} = JSON.parse(await peer.next());
    const initiator = await createInitiator(client, peerId);
    peer.send(initiator.hello());
    peer.send(await initiator.answer(await peer.next()));
    peer.end();
    assert.equal(await peer.exit, 3, peer.errors());
    assert.match(peer.errors(), /Untrusted/);
  } finally {
    peer.child.kill();
  }
});

test('a Rust initiator authenticates a JavaScript responder and they exchange records', async t => {
  if (!cargo) return unavailable(t, 'cargo');
  const server = await generateIdentity();
  const peer = startPeer('initiator');
  try {
    const {peerId: clientId} = JSON.parse(await peer.next());
    const responder = await createResponder(server, {allowClient: async id => id === clientId});
    peer.send(server.peerId);
    peer.send(await responder.challenge(await peer.next()));
    const accepted = await responder.accept(await peer.next());
    assert.equal(accepted.peerId, clientId);
    peer.send(accepted.reply);
    assert.equal(await peer.next(), toHex(accepted.session.id));
    const opened = await accepted.session.open(fromHex(await peer.next()));
    assert.equal(new TextDecoder().decode(opened), 'ping from rust');
    peer.send(toHex(await accepted.session.seal('echo:ping from rust')));
    assert.equal(await peer.next(), 'echo:ping from rust');
    assert.equal(await accepted.session.open(fromHex(await peer.next())), null);
    accepted.session.verifyEnd();
    peer.send(toHex(await accepted.session.close()));
    assert.equal(await peer.exit, 0, peer.errors());
  } finally {
    peer.child.kill();
  }
});

test('a Rust initiator refuses a JavaScript responder it did not pin', async t => {
  if (!cargo) return unavailable(t, 'cargo');
  const server = await generateIdentity();
  const peer = startPeer('initiator');
  try {
    await peer.next();
    const responder = await createResponder(server, {allowClient: async () => true});
    peer.send((await generateIdentity()).peerId);
    peer.send(await responder.challenge(await peer.next()));
    assert.equal(await peer.exit, 3, peer.errors());
    assert.match(peer.errors(), /Untrusted/);
  } finally {
    peer.child.kill();
  }
});

test('a Rust responder reports a stream a relay cut before the JavaScript close', async t => {
  if (!cargo) return unavailable(t, 'cargo');
  const client = await generateIdentity();
  const peer = startPeer('responder', client.peerId);
  try {
    const {peerId} = JSON.parse(await peer.next());
    const initiator = await createInitiator(client, peerId);
    peer.send(initiator.hello());
    peer.send(await initiator.answer(await peer.next()));
    const {session} = await initiator.finish(await peer.next());
    await peer.next();
    peer.send(toHex(await session.seal('the last message through')));
    await peer.next();
    await session.seal('never delivered');
    peer.end();
    assert.equal(await peer.exit, 4, peer.errors());
    assert.match(peer.errors(), /Truncated/);
  } finally {
    peer.child.kill();
  }
});
