/// Identities whose private keys are held in this process's memory, for tests and tools.
///
/// A product does not use these: its identity key lives in a platform key store, a browser's WebCrypto or a device, and it gives
/// `AcpSecretProver.signing` and `AcpSecretProver.agreeing` functions that call into it. Everything here keeps the raw key where a
/// memory dump can read it, ECDSA runs in pure Dart and is not constant-time, and its signatures are deterministic (RFC 6979).
library;

import 'dart:typed_data';

import 'package:pointycastle/export.dart' as pc;
import 'package:cryptography/cryptography.dart' as cr;

import 'secret.dart';
import 'src/hpke.dart' as hpke;

final _p256 = pc.ECDomainParameters('prime256v1');

Uint8List _bigBytes(BigInt value) {
  final out = Uint8List(32);
  for (var i = 31; i >= 0; i--) {
    out[i] = (value & BigInt.from(0xff)).toInt();
    value = value >> 8;
  }
  return out;
}

BigInt _bigOf(List<int> bytes) =>
    bytes.fold(BigInt.zero, (value, byte) => (value << 8) | BigInt.from(byte));

const _p256Prefix = [
  0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, //
  0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03,
  0x42, 0x00,
];
const _x25519Prefix = [
  0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x6e, 0x03, 0x21, 0x00, //
];

/// The P-256 `SubjectPublicKeyInfo` of a private scalar.
Uint8List ecdsaP256Spki(List<int> scalar) {
  final d = _bigOf(scalar);
  if (scalar.length != 32 || d == BigInt.zero || d >= _p256.n) {
    throw const AcpSecretException(
      AcpSecretFailure.key,
      'not a P-256 private scalar',
    );
  }
  return Uint8List.fromList([
    ..._p256Prefix,
    ...(_p256.G * d)!.getEncoded(false),
  ]);
}

/// A signer for a P-256 private scalar (32 bytes): deterministic, 64 bytes, `s` as the signing gives it.
AcpSecretSigner ecdsaP256Signer(List<int> scalar) {
  ecdsaP256Spki(scalar);
  final d = _bigOf(scalar);
  return (message) async {
    final signer = pc.ECDSASigner(
      pc.SHA256Digest(),
      pc.HMac(pc.SHA256Digest(), 64),
    )..init(true, pc.PrivateKeyParameter(pc.ECPrivateKey(d, _p256)));
    final signature = signer.generateSignature(message) as pc.ECSignature;
    return Uint8List.fromList([
      ..._bigBytes(signature.r),
      ..._bigBytes(signature.s),
    ]);
  };
}

/// An identity that signs, from its P-256 private scalar (32 bytes).
AcpSecretProver ecdsaP256Prover(List<int> scalar) => AcpSecretProver.signing(
  spki: ecdsaP256Spki(scalar),
  sign: ecdsaP256Signer(scalar),
);

/// The `SubjectPublicKeyInfo` of an X25519 private key (32 bytes).
Future<Uint8List> x25519Spki(List<int> secret) async =>
    Uint8List.fromList([..._x25519Prefix, ...await hpke.x25519Public(secret)]);

/// A key agreement for an X25519 private key (32 bytes).
AcpSecretAgreement x25519Agreement(List<int> secret) {
  final key = Uint8List.fromList(secret);
  return (peer) async {
    final pair = await cr.X25519().newKeyPairFromSeed(key);
    final shared = await cr.X25519().sharedSecretKey(
      keyPair: pair,
      remotePublicKey: cr.SimplePublicKey(peer, type: cr.KeyPairType.x25519),
    );
    return Uint8List.fromList(await shared.extractBytes());
  };
}

/// An X25519 identity, from its private key (32 bytes), for the collector whose `SubjectPublicKeyInfo` is [collectorSpki].
Future<AcpSecretProver> x25519Prover(
  List<int> secret,
  List<int> collectorSpki,
) async => AcpSecretProver.agreeing(
  spki: await x25519Spki(secret),
  agree: x25519Agreement(secret),
  collectorSpki: collectorSpki,
);
