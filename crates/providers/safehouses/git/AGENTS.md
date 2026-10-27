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
- **From a checkpoint** (`checkpoint = Some(saved)` with `isolate = true` and no `reopen`):
  `git fetch <saved.remote> refs/heads/<saved.branch>` in `source`, then a new worktree on a
  new branch named `saved.branch` at `saved.commit` (or the fetched tip when that commit is
  gone). The base is `saved.base` when it is an ancestor of the commit, else the commit
  itself. A local branch of that name already in `source` is refused by Git.
- **A clone** (`clone = Some(SafeHouseClone { path, create_source, git_dir })`, which
  overrides everything above): `source` is a repository URL (`file://`, `ssh://`, `git://`,
  `http(s)://`), `user@host:path` or a local repository, bare or not; a remote helper
  (`ext::`, `fd::`) or another scheme is refused before Git runs. With `create_source` and a
  local path or `file://` URL that does not exist, `git init --bare` makes it first, its
  `HEAD` naming `spec.base`. Then `git clone -- <source> <path>` (the directory must not
  exist; its parents are made; with `git_dir`, `--separate-git-dir=<git_dir>`, which must not
  exist either, and the `.git` pointer file is removed, so the working tree holds no `.git`)
  and `git checkout -B <base> --track origin/<base>`. Every command of a clone, the clone
  itself included, names `--git-dir` and `--work-tree` outright and runs with
  `core.hooksPath` set to a directory that does not exist, `core.fsmonitor=false` and
  `commit.gpgsign=false`; the Station user's global and system configuration still apply
  (credential helpers). A
  repository with no branch at all leaves `<base>` unborn (the base is empty); one with
  branches but not `<base>` is refused and the clone removed. Remote repositories are reached
  with the Station user's Git credentials.

## Session behaviour

- `info()`: path, branch, base, `supports_history = true`, and `isolated`.
- `changes()`: commits in `<base>..HEAD`, newest first, with full ID, `Name <email>` author
  and committer, and full message.
- `finish()`: an isolated worktree with no commits and a clean status is removed together
  with its branch (`kept = false`); otherwise it is kept and the report lists
  `"<short id> <subject>"` per commit. An in-place session is never removed
  (`kept = true`). A path that has disappeared reports `kept = false`.
- `checkpoint(CheckpointSpec)`: builds a tree of the worktree including uncommitted and
  untracked (not ignored) files in a private index, commits it on top of `HEAD` with
  `spec.message` (author and committer `spec.author_*` when both are set, else Git's
  configured identity), and `git push --force <remote> <commit>:refs/heads/<branch>`. With
  nothing uncommitted the commit is `HEAD` itself. The worktree, its index and `HEAD` are
  untouched. Answers the remote, branch, commit and base. An in-place session has no branch
  and is refused.
- `checkpoint(CheckpointSpec { branch: Some(b), fallback, .. })`, meant for a clone: commits
  everything in the working directory itself (`git add --all`, `git commit --no-verify` with
  `commit.gpgsign=false`, author and committer `spec.author_*`), so `HEAD` moves, then
  `git push <remote> HEAD:refs/heads/<b>` without force. A refused push fetches `<b>`,
  rebases onto it and pushes once more; when that fails too (a conflict aborts the rebase),
  `HEAD` is force-pushed to `fallback` and the answer's `branch` says so, or, without a
  fallback, the first refusal is the error. Nothing to commit and `HEAD` already on
  `<remote>/<b>` pushes nothing; an unborn `HEAD` with nothing written answers an empty
  `commit`.
- `exports()`: first `git fetch --prune --no-tags <remote>` for every configured remote (a
  remote that cannot be reached fails the call), then `unexported` lists `"<short id>
  <subject>"` for each commit in `<base>..HEAD` that none of those remotes' branches holds
  (`--not --remotes=<remote>/*`), so a stale or locally written remote-tracking ref never
  makes work read as exported; a commit reachable only from a tag counts as unexported.
  `.gitmodules` adds a line saying submodules are not checked, and `filter=lfs` in
  `.gitattributes` a line saying LFS objects are not checked: either makes the work read as
  unexported. `dirty` is a non-empty `git status --porcelain --untracked-files=all
  --ignore-submodules=none`, whatever `status.showUntrackedFiles` says. `ignored` lists the
  paths `git status --ignored` reports, a directory ignored whole as one entry (`target`), for
  the caller to judge. Each call reaches the network once per remote. A path that has
  disappeared exports nothing.
- `discard()`: `git worktree remove --force --force` and `git branch -D` of the Mission's
  branch, whatever they hold; what is already gone is skipped. A clone's directory is
  removed, read-only object files included, and so is its separate git directory. An in-place
  session is refused.
- `finish()` on a clone: removed when clean and every commit is on an `origin` branch, else
  kept and its unpushed commits listed.

## Gotchas

- A malformed spec is `GUATIAO_ERR_BAD_VALUE`; a Git failure is `GUATIAO_ERR_INTERNAL`
  carrying Git's own message.
- A Mission's `source` is a local checkout; only a `clone` spec takes a URL.
- After renaming or removing a provider library, clean the plugin directory: a Station
  loads every provider library it finds.
