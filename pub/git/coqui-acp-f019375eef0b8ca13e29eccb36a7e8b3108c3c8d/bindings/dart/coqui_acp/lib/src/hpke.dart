/// HPKE (RFC 9180), base mode, DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, AES-128-GCM, composed from the `cryptography` package's
/// X25519, HMAC and AES-GCM. This is the secret-prompt extension's suite and nothing more: one message at a time, sequence 0.
library;

import 'dart:convert';
import 'dart:typed_data';

import 'package:cryptography/cryptography.dart' as cr;

/// What the suite refused: a key of low order, or a message that does not open. It says nothing of the message.
final class HpkeFailure implements Exception {
  const HpkeFailure(this.reason);

  final String reason;

  @override
  String toString() => 'HpkeFailure($reason)';
}

/// An encapsulated key and a ciphertext with its 16-byte tag.
final class HpkeSealed {
  const HpkeSealed(this.enc, this.ct);

  final Uint8List enc, ct;
}

final _hmacAlgorithm = cr.Hmac.sha256();
final _x25519 = cr.X25519();
final _aesGcm = cr.AesGcm.with128bits();
final _kemSuite = Uint8List.fromList([...ascii.encode('KEM'), 0, 0x20]);
final _hpkeSuite = Uint8List.fromList([
  ...ascii.encode('HPKE'),
  0,
  0x20,
  0,
  1,
  0,
  1,
]);

Uint8List _concat(Iterable<List<int>> parts) {
  final out = BytesBuilder(copy: false);
  parts.forEach(out.add);
  return out.toBytes();
}

/// HMAC-SHA-256.
Future<Uint8List> hmacSha256(List<int> key, List<int> data) async =>
    Uint8List.fromList(
      (await _hmacAlgorithm.calculateMac(
        data,
        secretKey: cr.SecretKey(key),
      )).bytes,
    );

/// HKDF-Expand for at most one block, which is all the suite asks for: 32, 16 and 12 bytes.
Future<Uint8List> expandOne(List<int> prk, List<int> info, int length) async {
  assert(length <= 32);
  final block = await hmacSha256(prk, [...info, 1]);
  return Uint8List.sublistView(block, 0, length);
}

Future<Uint8List> _labeledExtract(
  List<int> suite,
  List<int> salt,
  String label,
  List<int> ikm,
) => hmacSha256(
  salt.isEmpty ? Uint8List(32) : salt,
  _concat([ascii.encode('HPKE-v1'), suite, ascii.encode(label), ikm]),
);

Future<Uint8List> _labeledExpand(
  List<int> suite,
  List<int> prk,
  String label,
  List<int> info,
  int length,
) => expandOne(
  prk,
  _concat([
    (ByteData(2)..setUint16(0, length)).buffer.asUint8List(),
    ascii.encode('HPKE-v1'),
    suite,
    ascii.encode(label),
    info,
  ]),
  length,
);

/// DeriveKeyPair for DHKEM(X25519): the private key of [ikm].
Future<Uint8List> deriveKey(List<int> ikm) async {
  final prk = await _labeledExtract(_kemSuite, const [], 'dkp_prk', ikm);
  return _labeledExpand(_kemSuite, prk, 'sk', const [], 32);
}

/// The X25519 public key of [secret].
Future<Uint8List> x25519Public(List<int> secret) async {
  final pair = await _x25519.newKeyPairFromSeed(secret);
  return Uint8List.fromList((await pair.extractPublicKey()).bytes);
}

/// The X25519 Diffie-Hellman of [secret] and [publicKey]; a result of low order is refused.
Future<Uint8List> x25519Dh(List<int> secret, List<int> publicKey) async {
  final Uint8List bytes;
  try {
    final pair = await _x25519.newKeyPairFromSeed(secret);
    final shared = await _x25519.sharedSecretKey(
      keyPair: pair,
      remotePublicKey: cr.SimplePublicKey(
        publicKey,
        type: cr.KeyPairType.x25519,
      ),
    );
    bytes = Uint8List.fromList(await shared.extractBytes());
  } on Object {
    // A browser's WebCrypto refuses a key of low order by throwing; the pure Dart code returns zeros.
    throw const HpkeFailure('a Diffie-Hellman key of low order');
  }
  if (bytes.every((byte) => byte == 0)) {
    throw const HpkeFailure('a Diffie-Hellman key of low order');
  }
  return bytes;
}

final class _Schedule {
  const _Schedule(this.key, this.nonce);

  final Uint8List key, nonce;
}

Future<_Schedule> _schedule(List<int> sharedSecret, List<int> info) async {
  final pskIdHash = await _labeledExtract(
    _hpkeSuite,
    const [],
    'psk_id_hash',
    const [],
  );
  final infoHash = await _labeledExtract(
    _hpkeSuite,
    const [],
    'info_hash',
    info,
  );
  final context = _concat([
    [0],
    pskIdHash,
    infoHash,
  ]);
  final secret = await _labeledExtract(
    _hpkeSuite,
    sharedSecret,
    'secret',
    const [],
  );
  return _Schedule(
    await _labeledExpand(_hpkeSuite, secret, 'key', context, 16),
    await _labeledExpand(_hpkeSuite, secret, 'base_nonce', context, 12),
  );
}

Future<Uint8List> _sharedSecret(List<int> dh, List<int> kemContext) async {
  final prk = await _labeledExtract(_kemSuite, const [], 'eae_prk', dh);
  return _labeledExpand(_kemSuite, prk, 'shared_secret', kemContext, 32);
}

/// Single-shot seal to [publicKey] with the ephemeral key derived from [ikm] (32 random bytes).
Future<HpkeSealed> hpkeSeal(
  List<int> publicKey,
  List<int> info,
  List<int> aad,
  List<int> plaintext,
  List<int> ikm,
) async {
  final secretE = await deriveKey(ikm);
  final enc = await x25519Public(secretE);
  final shared = await _sharedSecret(await x25519Dh(secretE, publicKey), [
    ...enc,
    ...publicKey,
  ]);
  final schedule = await _schedule(shared, info);
  final box = await _aesGcm.encrypt(
    plaintext,
    secretKey: cr.SecretKey(schedule.key),
    nonce: schedule.nonce,
    aad: aad,
  );
  return HpkeSealed(enc, _concat([box.cipherText, box.mac.bytes]));
}

/// Single-shot open with [secretKey] (the private key [deriveKey] gave), or an [HpkeFailure].
Future<Uint8List> hpkeOpen(
  List<int> secretKey,
  List<int> info,
  List<int> aad,
  HpkeSealed sealed,
) async {
  try {
    final publicKey = await x25519Public(secretKey);
    final shared = await _sharedSecret(await x25519Dh(secretKey, sealed.enc), [
      ...sealed.enc,
      ...publicKey,
    ]);
    final schedule = await _schedule(shared, info);
    final split = sealed.ct.length - 16;
    final plain = await _aesGcm.decrypt(
      cr.SecretBox(
        sealed.ct.sublist(0, split),
        nonce: schedule.nonce,
        mac: cr.Mac(sealed.ct.sublist(split)),
      ),
      secretKey: cr.SecretKey(schedule.key),
      aad: aad,
    );
    return Uint8List.fromList(plain);
  } on Object {
    throw const HpkeFailure('the message does not open');
  }
}
