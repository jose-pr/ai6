/**
 * The WebCrypto method of coqui-peer: a mutual P-256 key proof ending in an encrypted session.
 * Dependency-free; needs only `crypto.subtle`, so a browser needs a secure context.
 * The wire format is PROTOCOL.md, and only `session` records are tied to the proven peers.
 */
export const PROFILE = 'dev.boriken.coqui.peer-webcrypto/v1';
export const METHOD = 'webcrypto';
export const DEFAULT_MAX_MESSAGE = 3072;
// What one direction may carry before a new handshake: 2^38, against the 2^38.5 bytes of RFC 8446
// section 5.5's 2^24.5 full-size AES-GCM records. Each record counts its length and 32.
export const DEFAULT_SESSION_LIMIT = 2 ** 38;

const VERSION = 1;
const NONCE_LEN = 32;
const POINT_LEN = 65;
const SIGNATURE_LEN = 64;
const SPKI_LEN = 91;
const INNER_DATA = 1;
const INNER_CLOSE = 2;
const TAG_LEN = 16;
// A close record: its one inner byte and the tag. It stays possible at the limit.
const CLOSE_LEN = 1 + TAG_LEN;
const RECORD_OVERHEAD = 32;
// A P-256 SubjectPublicKeyInfo holding an uncompressed point is this prefix and 64 coordinate bytes,
// so comparing it leaves no ASN.1 for two parsers to read differently.
const SPKI_PREFIX = Uint8Array.from([
  0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
  0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00, 0x04,
]);
const PEER_ID = /^sha256:[a-f0-9]{64}$/;
const ECDSA = {name: 'ECDSA', namedCurve: 'P-256'};
const ECDH = {name: 'ECDH', namedCurve: 'P-256'};
const SIGN = {name: 'ECDSA', hash: 'SHA-256'};
const encoder = new TextEncoder();
const decoder = new TextDecoder('utf-8', {fatal: true, ignoreBOM: true});

function subtle() {
  if (!globalThis.crypto?.subtle || !globalThis.crypto.getRandomValues) {
    throw new Error('Secure-context WebCrypto is required');
  }
  return globalThis.crypto.subtle;
}

function b64(bytes) {
  let text = '';
  for (const byte of bytes) text += String.fromCharCode(byte);
  return btoa(text).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
}

function un64(text, length) {
  if (typeof text !== 'string' || !/^[A-Za-z0-9_-]+$/.test(text) ||
      text.length !== Math.ceil(length * 4 / 3)) {
    throw new Error('Invalid peer message');
  }
  let raw;
  try {
    raw = atob(text.replaceAll('-', '+').replaceAll('_', '/').padEnd(Math.ceil(text.length / 4) * 4, '='));
  } catch {
    throw new Error('Invalid peer message');
  }
  const bytes = Uint8Array.from(raw, char => char.charCodeAt(0));
  if (bytes.length !== length || b64(bytes) !== text) throw new Error('Invalid peer message');
  return bytes;
}

async function peerId(spki) {
  const digest = new Uint8Array(await subtle().digest('SHA-256', spki));
  return 'sha256:' + [...digest].map(byte => byte.toString(16).padStart(2, '0')).join('');
}

function hasSpkiShape(der) {
  return der.length === SPKI_LEN && SPKI_PREFIX.every((byte, index) => der[index] === byte);
}

async function verifyKey(encoded) {
  const der = un64(encoded, SPKI_LEN);
  if (!hasSpkiShape(der)) throw new Error('Invalid peer message');
  try {
    return await subtle().importKey('spki', der, ECDSA, false, ['verify']);
  } catch {
    throw new Error('Invalid peer message');
  }
}

async function ephemeralKey(encoded) {
  const point = un64(encoded, POINT_LEN);
  if (point[0] !== 4) throw new Error('Invalid peer message');
  try {
    return await subtle().importKey('raw', point, ECDH, false, []);
  } catch {
    throw new Error('Invalid peer message');
  }
}

function lengthPrefixed(fields) {
  const output = new Uint8Array(fields.reduce((total, field) => total + 4 + field.length, 0));
  const view = new DataView(output.buffer);
  let offset = 0;
  for (const field of fields) {
    view.setUint32(offset, field.length, false);
    offset += 4;
    output.set(field, offset);
    offset += field.length;
  }
  return output;
}

/** The length-prefixed fields signed, and with no role the session id's input. */
function encodeFields(role, hello, challenge, binding) {
  const fields = [encoder.encode(PROFILE)];
  if (role !== null) fields.push(encoder.encode(role));
  fields.push(
    un64(hello.publicKey, SPKI_LEN), un64(challenge.publicKey, SPKI_LEN),
    un64(hello.nonce, NONCE_LEN), un64(challenge.nonce, NONCE_LEN),
    un64(hello.ephemeral, POINT_LEN), un64(challenge.ephemeral, POINT_LEN),
    binding,
  );
  return lengthPrefixed(fields);
}

async function sign(identity, role, hello, challenge, binding) {
  return b64(new Uint8Array(await subtle().sign(
    SIGN, identity.privateKey, encodeFields(role, hello, challenge, binding),
  )));
}

async function verify(key, signature, role, hello, challenge, binding) {
  return subtle().verify(
    SIGN, key, un64(signature, SIGNATURE_LEN), encodeFields(role, hello, challenge, binding),
  );
}

/** One encoding only: a message is refused unless it is exactly what `pack` gives for its values. */
function pack(type, contents) {
  return JSON.stringify({profile: PROFILE, version: VERSION, type, ...contents});
}

function parse(raw, type, members, maxMessage) {
  let text = raw;
  if (raw instanceof ArrayBuffer || ArrayBuffer.isView(raw)) {
    try {
      text = decoder.decode(raw);
    } catch {
      throw new Error('Invalid peer message');
    }
  }
  if (typeof text !== 'string') throw new Error('Invalid peer message');
  if (encoder.encode(text).length > maxMessage) throw new Error('Oversized peer message');
  let value;
  try {
    value = JSON.parse(text);
  } catch {
    throw new Error('Invalid peer message');
  }
  if (!value || typeof value !== 'object') throw new Error('Invalid peer message');
  const contents = {};
  for (const member of members) {
    if (typeof value[member] !== 'string') throw new Error('Invalid peer message');
    contents[member] = value[member];
  }
  if (pack(type, contents) !== text) throw new Error('Invalid peer message');
  return contents;
}

async function hkdf(secretKey, info, bytes) {
  return new Uint8Array(await subtle().deriveBits(
    {name: 'HKDF', hash: 'SHA-256', salt: new Uint8Array(), info}, secretKey, bytes * 8,
  ));
}

/** A copy taken now: the caller may reuse its buffer before the record is made. */
function bytesOf(input) {
  if (typeof input === 'string') return encoder.encode(input);
  if (ArrayBuffer.isView(input)) {
    return new Uint8Array(input.buffer, input.byteOffset, input.byteLength).slice();
  }
  if (input instanceof ArrayBuffer) return new Uint8Array(input).slice();
  throw new TypeError('Expected text or bytes');
}

/** The session both ends hold after the handshake: an id, and keys only they can derive. */
async function createSession(ownEphemeral, peerEphemeral, hello, challenge, settings, initiator) {
  const id = new Uint8Array(await subtle().digest('SHA-256', encodeFields(null, hello, challenge, settings.binding)));
  const shared = new Uint8Array(await subtle().deriveBits(
    {name: 'ECDH', public: await ephemeralKey(peerEphemeral)}, ownEphemeral, 256,
  ));
  const master = await subtle().importKey('raw', shared, 'HKDF', false, ['deriveBits']);
  shared.fill(0);
  const info = lengthPrefixed([encoder.encode(`${PROFILE}/keys`), id]);
  const material = await hkdf(master, info, 96);
  const aes = bytes => subtle().importKey('raw', bytes, 'AES-GCM', false, ['encrypt', 'decrypt']);
  const keys = {initiator: await aes(material.subarray(0, 32)), responder: await aes(material.subarray(32, 64))};
  let exporter = await subtle().importKey('raw', material.subarray(64), 'HKDF', false, ['deriveBits']);
  material.fill(0);
  const status = {failed: false, destroyed: false};
  const sending = direction(initiator ? keys.initiator : keys.responder, 'encrypt', settings.sessionLimit, status);
  const receiving = direction(initiator ? keys.responder : keys.initiator, 'decrypt', settings.sessionLimit, status);
  let closed = false;
  let peerClosed = false;
  return Object.freeze({
    id,
    /** One message (bytes or text) as one record; send records whole, in the order of these calls. */
    seal(message) {
      const bytes = bytesOf(message);
      if (closed) return Promise.reject(new Error('This side already closed its direction'));
      const inner = new Uint8Array(bytes.length + 1);
      inner[0] = INNER_DATA;
      inner.set(bytes, 1);
      return sending(inner, inner.length + TAG_LEN);
    },
    /** The record ending this direction, to send last. A peer that never gets it reports truncation. */
    close() {
      if (closed) return Promise.reject(new Error('This side already closed its direction'));
      closed = true;
      return sending(Uint8Array.of(INNER_CLOSE), 0);
    },
    /** The message of the next record, or null for the peer's close. A bad record ends the session. */
    open(record) {
      const bytes = bytesOf(record);
      const opened = receiving(bytes, bytes.length > CLOSE_LEN ? bytes.length : 0);
      return opened.then(inner => {
        if (peerClosed) {
          status.failed = true;
          throw new Error('The session is over');
        }
        if (inner[0] === INNER_DATA) return inner.slice(1);
        if (inner[0] === INNER_CLOSE && inner.length === 1) {
          peerClosed = true;
          return null;
        }
        status.failed = true;
        throw new Error('A session record does not open');
      });
    },
    /** Call when the transport ended; throws when the peer's close record never came. */
    verifyEnd() {
      if (status.destroyed || status.failed) throw new Error('The session is over');
      if (!peerClosed) throw new Error('The stream was truncated: it ended before the peer closed it');
    },
    /** `length` bytes only this session's two peers can derive; another label or length is unrelated. */
    async export(label, length) {
      if (status.destroyed) throw new Error('The session is over');
      const lengthField = new Uint8Array(4);
      new DataView(lengthField.buffer).setUint32(0, length, false);
      const prefix = lengthPrefixed([encoder.encode(`${PROFILE}/export`), encoder.encode(label)]);
      const exportInfo = new Uint8Array(prefix.length + 4);
      exportInfo.set(prefix);
      exportInfo.set(lengthField, prefix.length);
      return hkdf(exporter, exportInfo, length);
    },
    /** Drops every key this session holds; each later call fails. WebCrypto then frees them. */
    destroy() {
      status.destroyed = true;
      keys.initiator = null;
      keys.responder = null;
      exporter = null;
    },
  });
}

/** One direction. Calls take their sequence number, and resolve, in call order. */
function direction(key, operation, limit, state) {
  let sequence = 0n;
  let charged = 0;
  let queue = Promise.resolve();
  let cipherKey = key;
  return (input, length) => {
    if (state.destroyed) cipherKey = null;
    if (length > 0) {
      const total = charged + length + RECORD_OVERHEAD;
      if (total > limit) return Promise.reject(new Error('This direction reached its session limit'));
      charged = total;
    }
    const number = sequence;
    sequence += 1n;
    const run = queue.then(async () => {
      if (state.failed || state.destroyed || number >= 0xffffffffffffffffn) {
        throw new Error('The session is over');
      }
      const iv = new Uint8Array(12);
      new DataView(iv.buffer).setBigUint64(4, number, false);
      try {
        return new Uint8Array(await subtle()[operation]({name: 'AES-GCM', iv, tagLength: 128}, cipherKey, input));
      } catch {
        state.failed = true;
        throw new Error('A session record does not open');
      }
    });
    queue = run.catch(() => {});
    return run;
  };
}

function options({
  maxMessage = DEFAULT_MAX_MESSAGE, sessionLimit = DEFAULT_SESSION_LIMIT,
  binding = new Uint8Array(), random, ephemeral,
} = {}) {
  if (!Number.isInteger(maxMessage) || maxMessage < 0) throw new Error('maxMessage must be a whole number');
  if (!Number.isSafeInteger(sessionLimit) || sessionLimit < 0) throw new Error('sessionLimit must be a whole number');
  // Anything else would turn into another value here, or none, and bind to nothing.
  if (!(binding instanceof Uint8Array)) throw new TypeError('binding must be a Uint8Array or Buffer');
  return {
    maxMessage,
    sessionLimit,
    binding: binding.slice(),
    random: random ?? (bytes => globalThis.crypto.getRandomValues(bytes)),
    ephemeral: ephemeral ?? (() => subtle().generateKey(ECDH, false, ['deriveBits'])),
  };
}

async function newEphemeral(settings) {
  const pair = await settings.ephemeral();
  return {
    privateKey: pair.privateKey,
    encoded: b64(new Uint8Array(await subtle().exportKey('raw', pair.publicKey))),
  };
}

function newNonce(settings) {
  const bytes = new Uint8Array(NONCE_LEN);
  settings.random(bytes);
  return b64(bytes);
}

async function checkIdentity(identity) {
  const key = identity?.privateKey;
  if (typeof identity?.publicKey !== 'string' || key?.type !== 'private' || key.extractable !== false ||
      key.algorithm?.name !== 'ECDSA' || key.algorithm?.namedCurve !== 'P-256' ||
      !key.usages?.includes('sign') || !PEER_ID.test(identity.peerId ?? '')) {
    throw new Error('A non-extractable P-256 identity is required');
  }
  await verifyKey(identity.publicKey);
  if (await peerId(un64(identity.publicKey, SPKI_LEN)) !== identity.peerId) {
    throw new Error('A non-extractable P-256 identity is required');
  }
}

function identityOf(spki, privateKey, id) {
  return Object.freeze({method: METHOD, peerId: id, publicKey: b64(spki), privateKey});
}

/** A new identity whose private key cannot leave WebCrypto. */
export async function generateIdentity() {
  const pair = await subtle().generateKey(ECDSA, false, ['sign', 'verify']);
  const spki = new Uint8Array(await subtle().exportKey('spki', pair.publicKey));
  return identityOf(spki, pair.privateKey, await peerId(spki));
}

/** The identity of a PKCS#8 DER P-256 key, for a server; the result's private key is non-extractable. */
export async function importIdentity(pkcs8) {
  const usable = await subtle().importKey('pkcs8', pkcs8, ECDSA, true, ['sign']);
  const {d: _d, key_ops: _ops, ...publicJwk} = await subtle().exportKey('jwk', usable);
  const publicKey = await subtle().importKey('jwk', publicJwk, ECDSA, true, ['verify']);
  const spki = new Uint8Array(await subtle().exportKey('spki', publicKey));
  const privateKey = await subtle().importKey('pkcs8', pkcs8, ECDSA, false, ['sign']);
  return identityOf(spki, privateKey, await peerId(spki));
}

/**
 * Starts a handshake as the side that dials, trusting only `expectedPeerId`.
 * Options: `maxMessage`; `binding`, bytes of an outer channel both ends really read (a TLS exporter
 * where each ends that TLS itself, never through a reverse proxy); `random` and `ephemeral` replace
 * the generators, for test vectors only.
 */
export async function createInitiator(identity, expectedPeerId, config) {
  await checkIdentity(identity);
  if (!PEER_ID.test(expectedPeerId ?? '')) throw new Error('A pinned responder id is required');
  const settings = options(config);
  const nonce = newNonce(settings);
  let ephemeral = await newEphemeral(settings);
  const hello = {publicKey: identity.publicKey, ephemeral: ephemeral.encoded, nonce};
  let state = 'start';
  let challenge;
  let peerKey;
  return Object.freeze({
    hello() {
      if (state !== 'start') throw new Error('Unexpected hello');
      state = 'challenge';
      return pack('hello', hello);
    },
    async answer(raw) {
      if (state !== 'challenge') throw new Error('Unexpected challenge');
      state = 'failed';
      const message = parse(raw, 'challenge', ['publicKey', 'ephemeral', 'nonce', 'signature'], settings.maxMessage);
      if (message.publicKey === identity.publicKey) throw new Error('Invalid peer message');
      const key = await verifyKey(message.publicKey);
      const answered = {publicKey: message.publicKey, nonce: message.nonce, ephemeral: message.ephemeral};
      await ephemeralKey(message.ephemeral);
      if (!await verify(key, message.signature, 'server', hello, answered, settings.binding)) {
        throw new Error('Responder key proof failed');
      }
      if (await peerId(un64(message.publicKey, SPKI_LEN)) !== expectedPeerId) {
        throw new Error('Responder is not pinned');
      }
      const signature = await sign(identity, 'client', hello, answered, settings.binding);
      challenge = answered;
      peerKey = key;
      state = 'accept';
      return pack('proof', {signature});
    },
    async finish(raw) {
      if (state !== 'accept') throw new Error('Unexpected acceptance');
      state = 'failed';
      const {signature} = parse(raw, 'accept', ['signature'], settings.maxMessage);
      if (!await verify(peerKey, signature, 'accept', hello, challenge, settings.binding)) {
        throw new Error('Responder acceptance proof failed');
      }
      const session = await createSession(
        ephemeral.privateKey, challenge.ephemeral, hello, challenge, settings, true,
      );
      ephemeral = null;
      peerKey = null;
      state = 'done';
      return {peerId: expectedPeerId, session};
    },
  });
}

/**
 * Starts a handshake as the side that accepts. `allowClient(peerId)` runs after the peer proved its
 * key and must resolve to exactly `true` to admit it. Other options as for `createInitiator`.
 */
export async function createResponder(identity, {allowClient, ...config} = {}) {
  await checkIdentity(identity);
  if (typeof allowClient !== 'function') throw new Error('An explicit trust decision is required');
  const settings = options(config);
  let state = 'hello';
  let hello;
  let challenge;
  let clientKey;
  let ephemeral;
  return Object.freeze({
    async challenge(raw) {
      if (state !== 'hello') throw new Error('Unexpected hello');
      state = 'failed';
      const message = parse(raw, 'hello', ['publicKey', 'ephemeral', 'nonce'], settings.maxMessage);
      if (message.publicKey === identity.publicKey) throw new Error('Invalid peer message');
      const key = await verifyKey(message.publicKey);
      await ephemeralKey(message.ephemeral);
      un64(message.nonce, NONCE_LEN);
      const nonce = newNonce(settings);
      const own = await newEphemeral(settings);
      const answered = {publicKey: identity.publicKey, nonce, ephemeral: own.encoded};
      const signature = await sign(identity, 'server', message, answered, settings.binding);
      hello = message;
      challenge = answered;
      clientKey = key;
      ephemeral = own;
      state = 'proof';
      return pack('challenge', {publicKey: answered.publicKey, ephemeral: own.encoded, nonce, signature});
    },
    async accept(raw) {
      if (state !== 'proof') throw new Error('Unexpected client proof');
      state = 'failed';
      const {signature} = parse(raw, 'proof', ['signature'], settings.maxMessage);
      if (!await verify(clientKey, signature, 'client', hello, challenge, settings.binding)) {
        throw new Error('Client key proof failed');
      }
      const id = await peerId(un64(hello.publicKey, SPKI_LEN));
      if (await allowClient(id) !== true) throw new Error('Client is not paired');
      const session = await createSession(
        ephemeral.privateKey, hello.ephemeral, hello, challenge, settings, false,
      );
      const reply = pack('accept', {signature: await sign(identity, 'accept', hello, challenge, settings.binding)});
      ephemeral = null;
      clientKey = null;
      state = 'done';
      return {reply, peerId: id, session};
    },
  });
}

function requestResult(request) {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

/**
 * The identity kept in the origin-local IndexedDB database `name`, created when absent. The private
 * key is stored as a non-extractable CryptoKey. Each application names its own database.
 */
export async function loadOrCreateIdentity(name) {
  if (typeof name !== 'string' || !name) throw new Error('An identity store name is required');
  if (!globalThis.indexedDB) throw new Error('IndexedDB storage is unavailable');
  const opening = indexedDB.open(name, 1);
  opening.onupgradeneeded = () => opening.result.createObjectStore('keys');
  const db = await requestResult(opening);
  const read = () => requestResult(db.transaction('keys').objectStore('keys').get('default'));
  try {
    let stored = await read();
    if (!stored) {
      const fresh = await generateIdentity();
      // Another tab may get there first: `add` then fails, and every tab uses the one stored.
      await requestResult(db.transaction('keys', 'readwrite').objectStore('keys').add(fresh, 'default')).catch(() => {});
      stored = await read();
    }
    await checkIdentity(stored);
    return stored;
  } finally {
    db.close();
  }
}

export const profile = Object.freeze({id: PROFILE, version: VERSION, method: METHOD});
