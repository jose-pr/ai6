# `ai6-safehouse-copy` — API header

Header-file-style reference for the `ai6-safehouse-copy` crate: the `copy` safe-house
provider for AI6, so it can be used without reading its source. It ships inside the package
and is self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No Cargo features. MSRV: Rust 1.89. Depends on `ai6` (no default features) only. The crate
root exports `Copier`, which implements
`ai6::directorates::q::safehouse::SafeHouseProvider`; the library is both an `rlib` and the
Guatiao `cdylib` a Station loads. Provider ID: `copy`. No unsafe code, no external command.

## Behaviour of `open(SafeHouseSpec)`

- **Isolated** (`isolate = true`): `source` is copied recursively to
  `<state_root>/safe-houses/<operation>/<mission>` (names reduced to safe characters plus a
  hash). Symbolic links are skipped, so the copy never reaches outside the source.
  `state_root` must lie outside `source`, and the Mission path must not already exist.
  `base` and `branch` are ignored.
- **Re-attach** (`reopen = Some(report)`): the report must be `kept` and name the existing
  copy this Mission would get; nothing is copied.
- **In place** (`isolate = false`): the session works in `source` and never removes it.

## Session behaviour

- `info()`: the path, no branch, no base, `supports_history = false` and `isolated`.
- `changes()`: always empty. With no history the daemon records the Mission's cover report
  as `checkable = false`, never as a pass.
- `finish()`: the copy is always kept, because without history an untouched copy cannot be
  told from a changed one (`kept = true` while the path exists); the report has no changes.
- `exports()`: unsupported (`GUATIAO_ERR_UNSUPPORTED`): a copy cannot tell what of its work
  exists elsewhere, so AI6 treats all of it as unexported.
- `discard()`: removes the copy (a copy already gone is fine); in place it is refused with
  `GUATIAO_ERR_BAD_VALUE` and the source is untouched.

## Gotchas

- A malformed spec is `GUATIAO_ERR_BAD_VALUE`; a filesystem failure is `GUATIAO_ERR_INTERNAL`.
- AI6 removes a copy only through `discard()`, which for a copy always needs green ink (its
  work cannot be shown to exist elsewhere); otherwise delete finished ones under
  `<state>/safe-houses/`.
- After renaming or removing a provider library, clean the plugin directory: a Station
  loads every provider library it finds.
