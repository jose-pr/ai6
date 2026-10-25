# `ai6-safehouse-git` — API header

Header-file-style reference for the `ai6-safehouse-git` crate: the `git` safe-house
provider for AI6, so it can be used without reading its source. It ships inside the package
and is self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No Cargo features. MSRV: Rust 1.89. Depends on `ai6` (no default features) only. The crate
root exports `Git`, which implements
`ai6::directorates::q::safehouse::SafeHouseProvider`; the library is both an `rlib` and the
Guatiao `cdylib` a Station loads. Provider ID: `git`. No unsafe code. Needs the `git`
command on the `PATH` of the Station that loads it; every call has a 120 second deadline and
runs with `GIT_TERMINAL_PROMPT=0`.

## Behaviour of `open(SafeHouseSpec)`

- **Isolated** (`isolate = true`): `source` must be a Git work tree. The base is
  `spec.base` resolved to a commit. A worktree is added at
  `<state_root>/safe-houses/<operation>/<mission>` (names reduced to safe characters plus a
  hash) on a new branch from `spec.branch`, where `{id}`, `{slug}` (lowercased title, at
  most 40 characters) and `{operation}` are filled in and characters Git rejects become `-`.
  `state_root` must lie outside `source`, and the Mission path must not already exist.
- **Re-attach** (`reopen = Some(report)` with `isolate = true`): the report must be `kept`,
  name an existing worktree registered with `source`, whose checked-out branch equals
  `report.branch`, and whose `report.base` is a commit that is an ancestor of `HEAD`.
- **In place** (`isolate = false`): the session works in `source`. The base is the commit
  checked out now, or `report.base` when re-attaching (verified as an ancestor of `HEAD`).
  There is no branch.

## Session behaviour

- `info()`: path, branch, base, `supports_history = true`, and `isolated`.
- `changes()`: commits in `<base>..HEAD`, newest first, with full ID, `Name <email>` author
  and committer, and full message.
- `finish()`: an isolated worktree with no commits and a clean status is removed together
  with its branch (`kept = false`); otherwise it is kept and the report lists
  `"<short id> <subject>"` per commit. An in-place session is never removed
  (`kept = true`). A path that has disappeared reports `kept = false`.

## Gotchas

- A malformed spec is `GUATIAO_ERR_BAD_VALUE`; a Git failure is `GUATIAO_ERR_INTERNAL`
  carrying Git's own message.
- `source` is a local path; cloning a URL is not supported.
- After renaming or removing a provider library, clean the plugin directory: a Station
  loads every provider library it finds.
