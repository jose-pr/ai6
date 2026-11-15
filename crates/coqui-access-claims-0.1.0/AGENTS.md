# `coqui-access-claims` — API header

Header-file-style reference for the `coqui-access-claims` crate: every public item with its
signature, contract and gotchas. It ships inside the package and is self-contained.
Development documentation lives with the source at
<https://github.com/Boriken-Dev/coqui-access>.

MSRV Rust 1.89. No Cargo features. Depends on `coqui-access`, `coqui-peer-tls` (feature
`fields`), `x509-parser`, `sha2` and `data-encoding`. No `unsafe` code. Reads no file and no
environment variable.

```rust
pub fn of_certificate(der: &[u8]) -> Option<coqui_access::Claims>;
pub fn public_key_hash(der: &[u8]) -> Option<String>;        // "sha256:<hex>" of the SubjectPublicKeyInfo
pub fn public_key_info_hash(info: &[u8]) -> String;
```

- `of_certificate` returns the certificate's named fields as `coqui-peer-tls`'s
  `CertificateFields::to_map` writes them (`subject.cn`, `san.upn`, `san.email`, `san.dns`,
  `san.uri`, `san.ip`, `fingerprint`, ...) with `subject`, `issuer` and `serial` renamed
  `subject.dn`, `issuer.dn` and `serial.hex`, plus `serial.reversed` (the serial's bytes last
  first), `spki` and the DER in `Claims::certificate`. `None` when the DER does not parse.
- The claims are what the certificate says, not proof of anything: whoever builds a lookup from
  them must have had the transport prove the key (`secure`) or a trusted proxy vouch for it.
