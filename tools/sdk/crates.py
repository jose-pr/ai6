#!/usr/bin/env python3
"""Keeps a checkout of the deps branch's crates/ equal to Cargo.lock, and builds offline from it.

Usage:
  crates.py sync DEPS_DIR      make DEPS_DIR/crates match Cargo.lock: fetch, verify, remove
  crates.py check DEPS_DIR     fail, naming what is missing, changed or stale
  crates.py use DEPS_DIR [--out DIR] [--repo-config]
                               unpack DEPS_DIR/crates into a directory source outside the
                               repository, write a cargo config for it, and print how to use it
  crates.py refresh DEPS_DIR   sync; commit on DEPS_DIR's branch; repin its Dockerfiles and
                               tools/sdk/sdk.lock, each in a commit of its own

Every command takes --cargo-lock FILE to read another copy of the lock. Exits 0, or 1 with one line
per problem. Needs Python 3.11.4 or later, and git to fetch a git-sourced package.
tools/sdk/README.md says how these fit together.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import NamedTuple

ROOT = Path(__file__).resolve().parents[2]
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
# The address crates.io's own index gives in its config.json; AI6_SDK_CRATES_URL names a mirror.
CRATES_URL = "https://static.crates.io/crates/{name}/{name}-{version}.crate"
SUMS = "git/SHA256SUMS"
# All a crates/ this script made can hold, so pointing it at a source tree removes nothing.
KEPT_FILE = re.compile(r"^([^/]+\.crate|git/[^/]+\.tar\.gz|git/SHA256SUMS)$")
STAMP = ".ai6-crates"
CONFIG_MARK = "# Written by tools/sdk/crates.py."
DOCKERFILES = ("containers/Dockerfile.linux", "containers/Dockerfile.windows")


class Problem(Exception):
    """What stops a command, for the one line it prints."""


class Crate(NamedTuple):
    name: str
    version: str
    checksum: str

    @property
    def file(self) -> str:
        return f"{self.name}-{self.version}.crate"


class Repo(NamedTuple):
    url: str
    commit: str

    @property
    def name(self) -> str:
        return self.url.rstrip("/").rsplit("/", 1)[-1].removesuffix(".git")

    @property
    def file(self) -> str:
        return f"git/{self.name}-{self.commit}.tar.gz"


class Lock(NamedTuple):
    crates: list[Crate]
    # Each repository at a revision, with the (name, version) Cargo.lock takes from it.
    repos: dict[Repo, list[tuple[str, str]]]
    # Each git source as Cargo.lock spells it, which is what a [source] entry has to replace.
    sources: dict[str, Repo]


class Tree(NamedTuple):
    """What a crates/ holds: each file's SHA-256 by its path below crates/, and git/SHA256SUMS."""

    digests: dict[str, str]
    sums: dict[str, str]


def read_lock(path: Path) -> Lock:
    lock = Lock([], {}, {})
    for package in tomllib.loads(path.read_text(encoding="utf-8")).get("package", []):
        name, version, source = package["name"], package["version"], package.get("source")
        if source is None:
            continue
        if source == CRATES_IO:
            if "checksum" not in package:
                raise Problem(f"{path.name} gives {name} {version} no checksum")
            lock.crates.append(Crate(name, version, package["checksum"]))
        elif source.startswith("git+"):
            parts = urllib.parse.urlsplit(source.removeprefix("git+"))
            repo = Repo(f"{parts.scheme}://{parts.netloc}{parts.path}", parts.fragment)
            lock.sources[source] = repo
            lock.repos.setdefault(repo, []).append((name, version))
        else:
            raise Problem(f"{name} {version} comes from {source}, which only crates.io and git are")
    return lock


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def parse_sums(text: str) -> dict[str, str]:
    """The `sha256sum` format, with names relative to crates/git/."""
    return {f"git/{line.split()[1]}": line.split()[0] for line in text.splitlines() if line.strip()}


def read_dir(crates: Path) -> Tree:
    digests = {}
    if crates.is_dir():
        for path in sorted(crates.rglob("*")):
            if path.is_file():
                digests[path.relative_to(crates).as_posix()] = sha256(path.read_bytes())
    sums = crates / SUMS
    return Tree(digests, parse_sums(sums.read_text(encoding="utf-8")) if sums.is_file() else {})


def read_ref(checkout: Path, commit: str) -> Tree | None:
    """What a commit's crates/ holds, or None when the commit or its crates/ is not here."""
    probe = ["git", "-C", str(checkout), "cat-file", "-e", f"{commit}:crates"]
    if subprocess.run(probe, capture_output=True).returncode != 0:
        return None
    read = subprocess.Popen(
        ["git", "-C", str(checkout), "archive", "--format=tar", commit, "crates"],
        stdout=subprocess.PIPE,
    )
    digests, sums = {}, ""
    with tarfile.open(fileobj=read.stdout, mode="r|") as tar:
        for member in tar:
            if member.isfile():
                data = tar.extractfile(member).read()
                name = member.name.removeprefix("crates/")
                digests[name] = sha256(data)
                if name == SUMS:
                    sums = data.decode("utf-8")
    read.wait()
    return Tree(digests, parse_sums(sums))


def problems(lock: Lock, tree: Tree) -> list[str]:
    """One line for each thing keeping a crates/ from covering Cargo.lock, and nothing more."""
    found = []
    wanted = set()
    for crate in lock.crates:
        wanted.add(crate.file)
        have = tree.digests.get(crate.file)
        if have is None:
            found.append(f"crates/{crate.file}: missing, Cargo.lock names it")
        elif have != crate.checksum:
            found.append(f"crates/{crate.file}: SHA-256 {have}, Cargo.lock says {crate.checksum}")
    for repo in lock.repos:
        wanted.update((repo.file, SUMS))
        have = tree.digests.get(repo.file)
        listed = tree.sums.get(repo.file)
        if have is None:
            found.append(
                f"crates/{repo.file}: missing, Cargo.lock names {repo.name} at this commit"
            )
        elif listed is None:
            found.append(f"crates/{repo.file}: not in crates/{SUMS}")
        elif have != listed:
            found.append(f"crates/{repo.file}: SHA-256 {have}, crates/{SUMS} says {listed}")
    for name in sorted(set(tree.digests) - wanted):
        found.append(f"crates/{name}: stale, Cargo.lock does not name it")
    for name in sorted(set(tree.sums) - wanted):
        found.append(f"crates/{SUMS}: stale line for {name}")
    return found


def run_git(*args: str, cwd: Path | None = None) -> str:
    command = ["git", *(["-C", str(cwd)] if cwd else []), *args]
    done = subprocess.run(command, capture_output=True)
    if done.returncode != 0:
        detail = done.stderr.decode("utf-8", "replace").strip()
        raise Problem(f"git {' '.join(args)}: {detail}")
    return done.stdout.decode("utf-8").strip()


def download(url: str) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": "ai6-tools-sdk-crates"})
    failure: Exception | None = None
    for _ in range(3):
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                return response.read()
        except urllib.error.HTTPError as error:
            if error.code < 500:
                raise Problem(f"{url}: {error}") from error
            failure = error
        except OSError as error:
            failure = error
    raise Problem(f"{url}: {failure}")


def fetch_crate(crate: Crate) -> bytes:
    template = os.environ.get("AI6_SDK_CRATES_URL") or CRATES_URL
    # A version's build metadata has a + that a URL path would otherwise leave ambiguous.
    version = urllib.parse.quote(crate.version, safe="")
    data = download(template.format(name=crate.name, version=version))
    if sha256(data) != crate.checksum:
        raise Problem(
            f"refused {crate.file}: SHA-256 {sha256(data)}, Cargo.lock says {crate.checksum}"
        )
    return data


def archive_commit(repo: Repo) -> bytes:
    """An archive of exactly this commit's tree, with no history: git checks the hash it fetched."""
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        run_git("init", "-q", cwd=work)
        run_git("fetch", "-q", "--depth", "1", repo.url, repo.commit, cwd=work)
        if run_git("rev-parse", "FETCH_HEAD", cwd=work) != repo.commit:
            raise Problem(f"{repo.url} did not give commit {repo.commit}")
        probe = ["git", "-C", tmp, "cat-file", "-e", "FETCH_HEAD:.gitmodules"]
        if subprocess.run(probe, capture_output=True).returncode == 0:
            raise Problem(f"{repo.name} has submodules, which a source archive would leave out")
        made = subprocess.run(
            [
                "git",
                "-C",
                tmp,
                "-c",
                "core.autocrlf=false",
                "-c",
                "core.eol=lf",
                "archive",
                "--format=tar",
                f"--prefix={repo.name}-{repo.commit}/",
                "FETCH_HEAD",
            ],
            capture_output=True,
            check=True,
        )
    # mtime=0: the same tree gives the same bytes, whenever it is made.
    return gzip.compress(made.stdout, compresslevel=9, mtime=0)


def sync(deps_dir: Path, lock: Lock) -> list[str]:
    """Makes deps_dir/crates cover the lock and hold nothing else; returns a line per change."""
    crates = deps_dir / "crates"
    tree = read_dir(crates)
    unknown = sorted(name for name in tree.digests if not KEPT_FILE.match(name))
    if unknown:
        shown = ", ".join(unknown[:3])
        raise Problem(
            f"{crates} holds files this script does not make ({shown}): is it a deps checkout?"
        )
    changes = []
    (crates / "git").mkdir(parents=True, exist_ok=True)

    def changed(file: str, have: str | None) -> str:
        return f"{'replaced' if have else 'added'} {file}"

    stale = [c for c in lock.crates if tree.digests.get(c.file) != c.checksum]
    with ThreadPoolExecutor(max_workers=8) as pool:
        for crate, data in zip(stale, pool.map(fetch_crate, stale)):
            (crates / crate.file).write_bytes(data)
            changes.append(changed(crate.file, tree.digests.get(crate.file)))
    sums = {}
    for repo in lock.repos:
        have = tree.digests.get(repo.file)
        if have is None or have != tree.sums.get(repo.file):
            data = archive_commit(repo)
            (crates / repo.file).write_bytes(data)
            have_now = sha256(data)
            changes.append(changed(repo.file, have))
        else:
            have_now = have
        sums[repo.file] = have_now
    wanted = {c.file for c in lock.crates} | set(sums) | {SUMS}
    for name in sorted(set(tree.digests) - wanted):
        (crates / name).unlink()
        changes.append(f"removed {name}")
    if sums:
        text = "".join(
            f"{digest}  {name.removeprefix('git/')}\n" for name, digest in sorted(sums.items())
        )
        (crates / SUMS).write_bytes(text.encode("utf-8"))
    else:
        (crates / SUMS).unlink(missing_ok=True)
        (crates / "git").rmdir()
    return changes


def toml_value(value: object) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return str(value)
    if isinstance(value, str):
        # What JSON escapes is a TOML basic string too, as long as it writes no surrogate pairs.
        return json.dumps(value, ensure_ascii=False)
    if isinstance(value, list):
        return "[" + ", ".join(toml_value(item) for item in value) + "]"
    if isinstance(value, dict):
        pairs = (f"{toml_key(key)} = {toml_value(item)}" for key, item in value.items())
        return "{ " + ", ".join(pairs) + " }"
    raise Problem(f"cannot write {value!r} back as TOML")


def toml_key(key: str) -> str:
    return key if re.fullmatch(r"[A-Za-z0-9_-]+", key) else json.dumps(key, ensure_ascii=False)


def toml_table(table: dict, path: list[str], lines: list[str]) -> None:
    """Writes one table, its plain values before its subtables as TOML requires."""
    subtables = {key: item for key, item in table.items() if isinstance(item, dict)}
    if path:
        lines.append("[" + ".".join(toml_key(part) for part in path) + "]")
    for key, item in table.items():
        if key not in subtables:
            lines.append(f"{toml_key(key)} = {toml_value(item)}")
    for key, item in subtables.items():
        toml_table(item, [*path, key], lines)


def inherit_dependency(name: str, own: dict, workspace: dict, manifest: str) -> dict:
    """A `{ workspace = true }` dependency, with cargo's rules for what the member may add."""
    shared = workspace.get("dependencies", {}).get(name)
    if shared is None:
        raise Problem(f"{manifest} inherits dependency {name}, which its workspace does not list")
    merged = {"version": shared} if isinstance(shared, str) else dict(shared)
    features = list(merged.get("features", [])) + list(own.get("features", []))
    if features:
        merged["features"] = features
    if "optional" in own:
        merged["optional"] = own["optional"]
    for spelling in ("default-features", "default_features"):
        if (
            spelling in own
            and "default-features" not in merged
            and "default_features" not in merged
        ):
            merged[spelling] = own[spelling]
    return merged


def normalized_manifest(
    manifest: dict, root: dict, member_dir: Path, root_dir: Path, label: str
) -> dict:
    """A workspace member's manifest with nothing left to inherit, as `cargo package` would write.

    A directory source reads each package on its own, with no workspace above it. A `path`
    dependency stays: within a directory source it means this same source, wherever the path points.
    """
    workspace = root.get("workspace", {})
    shared = workspace.get("package", {})
    result = {
        k: v for k, v in manifest.items() if k not in ("workspace", "patch", "replace", "profile")
    }
    package = dict(result["package"])
    package.pop("workspace", None)
    for key, value in list(package.items()):
        if isinstance(value, dict) and value.get("workspace") is True:
            if key not in shared:
                raise Problem(f"{label} inherits package.{key}, which its workspace does not set")
            value = shared[key]
            if key in ("readme", "license-file") and isinstance(value, str):
                value = Path(os.path.relpath(root_dir / value, member_dir)).as_posix()
            package[key] = value
    result["package"] = package
    lints = result.get("lints")
    if isinstance(lints, dict) and lints.get("workspace") is True:
        result.pop("lints")
        if "lints" in workspace:
            result["lints"] = workspace["lints"]

    def inherit_tables(table: dict) -> None:
        for kind in (
            "dependencies",
            "dev-dependencies",
            "dev_dependencies",
            "build-dependencies",
            "build_dependencies",
        ):
            if kind in table:
                table[kind] = {
                    name: (
                        inherit_dependency(name, value, workspace, label)
                        if isinstance(value, dict) and value.get("workspace") is True
                        else value
                    )
                    for name, value in table[kind].items()
                }

    inherit_tables(result)
    for target in result.get("target", {}).values():
        inherit_tables(target)
    return result


def unpack_repo(
    repo: Repo, wanted: list[tuple[str, str]], archive: Path, into: Path, scratch: Path
) -> None:
    """Puts each package Cargo.lock takes from an archive in a directory of its own below `into`."""
    with tarfile.open(archive, mode="r:gz") as tar:
        tar.extractall(scratch, filter="data")
    tree = scratch / f"{repo.name}-{repo.commit}"
    manifests: dict[Path, dict] = {}
    for path in tree.rglob("Cargo.toml"):
        if any(part.startswith(".") for part in path.relative_to(tree).parts[:-1]):
            continue
        try:
            manifests[path.parent] = tomllib.loads(path.read_text(encoding="utf-8"))
        except (tomllib.TOMLDecodeError, UnicodeDecodeError):
            # A fixture, or anything else cargo would also skip when it searches a git source.
            continue

    def workspace_root(member: Path) -> Path | None:
        for directory in (member, *member.parents):
            if directory == tree.parent:
                return None
            if "workspace" in manifests.get(directory, {}):
                return directory
        return None

    for name, version in wanted:
        found = []
        for directory, manifest in manifests.items():
            package = manifest.get("package", {})
            if package.get("name") != name:
                continue
            root_dir = workspace_root(directory)
            root = manifests.get(root_dir, {}) if root_dir else {}
            shown = package.get("version")
            if isinstance(shown, dict):
                shown = root.get("workspace", {}).get("package", {}).get("version")
            if shown == version:
                found.append((directory, manifest, root_dir or directory, root))
        if len(found) != 1:
            raise Problem(f"{archive.name} has {len(found)} packages named {name} {version}")
        directory, manifest, root_dir, root = found[0]
        label = f"{repo.name}/{directory.relative_to(tree).as_posix()}"
        written = normalized_manifest(manifest, root, directory, root_dir, label)
        target = into / f"{name}-{version}"
        shutil.copytree(directory, target)
        lines = [f"# {label} at {repo.commit}, with what it inherits written out."]
        toml_table(written, [], lines)
        (target / "Cargo.toml").write_bytes(("\n".join(lines) + "\n").encode("utf-8"))
        # No files listed: there is nothing to compare, and git packages have no package checksum.
        (target / ".cargo-checksum.json").write_bytes(b'{"files":{},"package":null}')


def source_entry(source: str) -> list[str]:
    """The keys naming a git source as Cargo.lock spells it: its url and one of rev, tag, branch."""
    parts = urllib.parse.urlsplit(source.removeprefix("git+"))
    lines = [f"git = {toml_value(f'{parts.scheme}://{parts.netloc}{parts.path}')}"]
    for key, value in urllib.parse.parse_qsl(parts.query):
        lines.append(f"{key} = {toml_value(value)}")
    return lines


def config_text(lock: Lock, directories: dict[Repo, Path], registry: Path) -> str:
    lines = [
        f"{CONFIG_MARK} Every source Cargo.lock names, from a directory.",
        "[net]",
        "offline = true",
        "",
        "[source.crates-io]",
        'replace-with = "ai6-crates"',
        "",
        "[source.ai6-crates]",
        f"directory = {toml_value(registry.as_posix())}",
    ]
    # A directory of its own for each repository, since two sources may hold one name and version.
    aliases = {repo: f"ai6-git-{repo.name}-{repo.commit[:12]}" for repo in lock.repos}
    for repo, alias in aliases.items():
        lines += [
            "",
            f"[source.{alias}]",
            f"directory = {toml_value(directories[repo].as_posix())}",
        ]
    for source, repo in sorted(lock.sources.items()):
        lines += ["", f"[source.{toml_value(source)}]", *source_entry(source)]
        lines.append(f"replace-with = {toml_value(aliases[repo])}")
    return "\n".join(lines) + "\n"


def use(deps_dir: Path, lock: Lock, lock_path: Path, out: Path | None, repo_config: bool) -> None:
    crates = deps_dir / "crates"
    found = problems(lock, read_dir(crates))
    if found:
        raise Problem("; ".join(found[:5]) + f" ({len(found)} in all); run sync first")
    stamp = sha256(lock_path.read_bytes())
    if out is None:
        out = Path(tempfile.gettempdir()) / f"ai6-crates-{stamp[:12]}"
    out = out.resolve()
    if out == ROOT or ROOT in out.parents:
        raise Problem(f"{out} is inside the repository; --out names a directory outside it")
    home = out / "cargo-home"
    config = home / "config.toml"
    if not ((out / STAMP).is_file() and (out / STAMP).read_text(encoding="utf-8") == stamp):
        if out.exists() and any(out.iterdir()) and not (out / STAMP).is_file():
            raise Problem(f"{out} is not empty and is not a directory this command made")
        if out.exists():
            shutil.rmtree(out)
        registry = out / "registry"
        registry.mkdir(parents=True)
        for crate in lock.crates:
            with tarfile.open(crates / crate.file, mode="r:gz") as tar:
                tar.extractall(registry, filter="data")
            digest = json.dumps({"files": {}, "package": crate.checksum}, separators=(",", ":"))
            (registry / f"{crate.name}-{crate.version}" / ".cargo-checksum.json").write_bytes(
                digest.encode()
            )
        directories = {}
        for repo, wanted in lock.repos.items():
            directories[repo] = out / "git" / f"{repo.name}-{repo.commit[:12]}"
            directories[repo].mkdir(parents=True)
            with tempfile.TemporaryDirectory(dir=out) as scratch:
                unpack_repo(repo, wanted, crates / repo.file, directories[repo], Path(scratch))
        home.mkdir()
        config.write_bytes(config_text(lock, directories, registry).encode("utf-8"))
        (out / STAMP).write_bytes(stamp.encode())
    if repo_config:
        target = ROOT / ".cargo" / "config.toml"
        if target.exists() and not target.read_text(encoding="utf-8").startswith(CONFIG_MARK):
            raise Problem(f"{target} is not one this command wrote; it was left alone")
        target.parent.mkdir(exist_ok=True)
        target.write_bytes(config.read_bytes())
        print(f"wrote {target}; delete it to build from the network again")
    print(
        f"offline build ready: {len(lock.crates)} crates, {len(lock.repos)} git repositories: {out}"
    )
    print(f"  sh:          export CARGO_HOME='{home}' CARGO_NET_OFFLINE=true")
    print(f"  PowerShell:  $env:CARGO_HOME = '{home}'; $env:CARGO_NET_OFFLINE = 'true'")
    print(f"  or one command, keeping your CARGO_HOME:  cargo --config '{config}' build --locked")


def refresh(deps_dir: Path, lock: Lock, lock_path: Path) -> None:
    """Sync, then the commits that keep a pin true: deps' crates/, its Dockerfiles, then sdk.lock.

    The lock pins the crates/ commit, and the Dockerfiles naming that commit come after it, since a
    commit cannot hold its own hash.
    """
    if subprocess.run(
        ["git", "-C", str(deps_dir), "symbolic-ref", "-q", "HEAD"], capture_output=True
    ).returncode:
        raise Problem(f"{deps_dir} is not on a branch of the deps branch's history")
    dirty = run_git("status", "--porcelain", "--", ":!crates", cwd=deps_dir)
    if dirty:
        raise Problem(f"{deps_dir} has changes outside crates/: {dirty.splitlines()[0]}")
    changes = sync(deps_dir, lock)
    for change in changes:
        print(change)
    source_commit = run_git("rev-parse", "--short=8", "HEAD", cwd=ROOT)
    run_git("add", "-A", "--", "crates", cwd=deps_dir)
    committed = bool(run_git("diff", "--cached", "--name-only", "--", "crates", cwd=deps_dir))
    if committed:
        body = "\n".join(changes) if len(changes) <= 40 else f"{len(changes)} files changed."
        run_git(
            "commit",
            "-q",
            "-m",
            f"chore: crates/ for main at {source_commit}",
            "-m",
            body,
            cwd=deps_dir,
        )
    sdk_lock = ROOT / "tools/sdk/sdk.lock"
    text = sdk_lock.read_bytes().decode("utf-8")
    line = re.search(r"^app deps (\S+) ", text, re.MULTILINE)
    if not line:
        raise Problem(f"{sdk_lock} has no app deps line")
    pinned = read_ref(ROOT, line[1])
    if not committed and pinned is not None and not problems(lock, pinned):
        print(f"current: deps {line[1]} covers {lock_path.name}")
        return
    pin = run_git("rev-parse", "HEAD", cwd=deps_dir)
    for name in DOCKERFILES:
        path = deps_dir / name
        path.write_bytes(
            re.sub(
                rb"^ARG DEPS_REF=\S+$",
                f"ARG DEPS_REF={pin}".encode(),
                path.read_bytes(),
                flags=re.MULTILINE,
            )
        )
    run_git("add", "--", *DOCKERFILES, cwd=deps_dir)
    if run_git("diff", "--cached", "--name-only", cwd=deps_dir):
        run_git(
            "commit",
            "-q",
            "-m",
            f"chore: the containers default to deps at {pin[:8]}",
            cwd=deps_dir,
        )
    sdk_lock.write_bytes(text.replace(f"app deps {line[1]} ", f"app deps {pin} ").encode("utf-8"))
    if subprocess.run(
        ["git", "-C", str(ROOT), "cat-file", "-e", pin], capture_output=True
    ).returncode:
        run_git("fetch", "-q", str(deps_dir), "HEAD", cwd=ROOT)
    run_git(
        "commit",
        "-q",
        "-m",
        f"chore: sdk.lock pins deps at {pin[:8]}",
        "--",
        "tools/sdk/sdk.lock",
        cwd=ROOT,
    )
    found = problems(lock, read_ref(ROOT, pin) or Tree({}, {}))
    if found:
        raise Problem(f"deps {pin} does not cover {lock_path.name}: {found[0]}")
    print(
        f"pinned deps {pin}; push {deps_dir}'s branch to deps first, not rebased, then this one"
    )


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Keep crates/ on the deps branch equal to Cargo.lock."
    )
    parser.add_argument("command", choices=("sync", "check", "use", "refresh"))
    parser.add_argument("deps_dir", type=Path, metavar="DEPS_DIR")
    parser.add_argument("--cargo-lock", type=Path, default=ROOT / "Cargo.lock")
    parser.add_argument(
        "--out", type=Path, help="use: where to unpack (default: a new temporary directory)"
    )
    parser.add_argument(
        "--repo-config",
        action="store_true",
        help="use: also write this checkout's .cargo/config.toml",
    )
    args = parser.parse_args()
    try:
        lock = read_lock(args.cargo_lock)
        if args.command == "sync":
            changes = sync(args.deps_dir, lock)
            for change in changes:
                print(change)
            print(
                f"ok: {len(lock.crates)} crates, {len(lock.repos)} git archives, "
                f"{len(changes)} changed"
            )
        elif args.command == "check":
            found = problems(lock, read_dir(args.deps_dir / "crates"))
            for line in found:
                print(line, file=sys.stderr)
            if found:
                return 1
            print(
                f"ok: {len(lock.crates)} crates, {len(lock.repos)} git archives "
                f"cover {args.cargo_lock.name}"
            )
        elif args.command == "use":
            use(args.deps_dir, lock, args.cargo_lock, args.out, args.repo_config)
        else:
            refresh(args.deps_dir, lock, args.cargo_lock)
    except Problem as problem:
        print(f"crates: {problem}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
