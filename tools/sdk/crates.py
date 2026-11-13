#!/usr/bin/env python3
"""Keeps a checkout of the deps branch's crates/ equal to Cargo.lock, and builds offline from it.

crates/ is a cargo vendor directory: each package Cargo.lock names, from crates.io or git, is a
directory `<name>-<version>/` of unpacked source with a `.cargo-checksum.json`. Tests, examples,
benchmarks, fixtures and anything an archive scanner would refuse are left out, since compiling
reads none of them.

Usage:
  crates.py sync DEPS_DIR      make DEPS_DIR/crates match Cargo.lock: fetch, verify, remove
  crates.py check DEPS_DIR     fail, naming what is missing, changed or stale
  crates.py use DEPS_DIR [--out DIR] [--repo-config]
                               write a cargo config that replaces every source by DEPS_DIR/crates,
                               and print how to use it
  crates.py refresh DEPS_DIR   sync; commit on DEPS_DIR's branch; repin its Dockerfiles and
                               tools/sdk/sdk.lock, each in a commit of its own

Every command takes --cargo-lock FILE to read another copy of the lock. Exits 0, or 1 with one line
per problem. Needs Python 3.11.4 or later, and git to fetch a git-sourced package.
tools/sdk/README.md says how these fit together.
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import io
import json
import math
import os
import posixpath
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
from typing import Iterator, NamedTuple

ROOT = Path(__file__).resolve().parents[2]
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
# The address crates.io's own index gives in its config.json; AI6_SDK_CRATES_URL names a mirror.
CRATES_URL = "https://static.crates.io/crates/{name}/{name}-{version}.crate"
CHECKSUM = ".cargo-checksum.json"
# What an older crates/ held (.crate files, source archives); sync removes it.
LEGACY_FILE = re.compile(r"^([^/]+\.crate|git/[^/]+\.tar\.gz|git/SHA256SUMS)$")
CONFIG_MARK = "# Written by tools/sdk/crates.py."
SOURCE = "ai6-vendored"
DOCKERFILES = ("containers/Dockerfile.linux", "containers/Dockerfile.windows")
# Directories at a package's top that compiling never reads, and at any depth the ones that hold
# only test data.
DROP_TOP = {
    "test", "tests", "example", "examples", "bench", "benches", "benchmark", "benchmarks", "fuzz",
    "testdata", "test-data", "test_data", "fixture", "fixtures",
}  # fmt: skip
DROP_ANY = {"testdata", "test-data", "test_data", "fixture", "fixtures"}
# A name Windows cannot create, which would make the checkout differ between platforms.
INCLUDE = re.compile(r'include_(?:str|bytes)!\s*\(\s*"([^"]+)"\s*,?\s*\)|\binclude!\s*\(\s*"([^"]+)"\s*,?\s*\)')
INCLUDE_BYTES = re.compile(r'(\*\s*)?include_bytes!\s*\(\s*"([^"]+)"\s*,?\s*\)')
# A source file only tests, fuzzing or benchmarks compile; what it includes is not worth carrying.
TEST_ONLY = re.compile(r"(^|/)(fuzz|tests?|test_vectors|test-vectors|benches|bench|examples?)(\.rs)?(/|$)")
# The big-endian copy of a table; no target of ours has that byte order.
BIG_ENDIAN = re.compile(r"(^|[._-])bigendian([._-]|$)")
# The markers of a PEM private key, which one dash fewer no longer reads as.
PEM_MARKER = re.compile(rb"-----(BEGIN|END) ((?:[A-Z0-9-]+ )?PRIVATE KEY)-----")
PEM_REPLACEMENT = rb"----\1 \2-----"
RESERVED = re.compile(r"^(con|prn|aux|nul|com[0-9]|lpt[0-9])(\..*)?$", re.IGNORECASE)

# What an archive scanner refuses, judged by a file's leading bytes (the same list as the
# scanner this branch is held to).
FLAG_MAGIC = [
    (b"PK\x03\x04", "zip"), (b"PK\x05\x06", "zip"), (b"PK\x07\x08", "zip"),
    (b"\x1f\x8b", "gzip"), (b"BZh", "bzip2"), (b"\xfd7zXZ\x00", "xz"),
    (b"7z\xbc\xaf\x27\x1c", "7z"), (b"Rar!\x1a\x07", "rar"), (b"\x28\xb5\x2f\xfd", "zstd"),
    (b"\x04\x22\x4d\x18", "lz4"), (b"MSCF", "cab"), (b"Cr24", "crx"), (b"!<arch>", "ar/deb"),
    (b"\xed\xab\xee\xdb", "rpm"), (b"Salted__", "openssl-encrypted"),
    (b"age-encryption.org", "age-encrypted"), (b"-----BEGIN PGP MESSAGE", "pgp-encrypted"),
    (b"MZ", "windows executable"), (b"\x7fELF", "elf executable"),
    (b"\xcf\xfa\xed\xfe", "mach-o executable"), (b"\xfe\xed\xfa\xcf", "mach-o executable"),
    (b"\xca\xfe\xba\xbe", "mach-o/java class"), (b"PACK", "git pack"),
    (b"\xfftOc", "git pack index"), (b"RIDX", "git reverse index"),
    (b"\x30\x82", "der (certificate or key)"),
]  # fmt: skip
MEDIA = [
    b"\x89PNG", b"GIF8", b"\xff\xd8\xff", b"RIFF", b"\x00\x00\x01\x00", b"wOFF", b"wOF2",
    b"\x00\x01\x00\x00", b"OTTO",
]  # fmt: skip
KEY_EXT = re.compile(r"\.(pfx|p12|jks|keystore|der|key)$", re.IGNORECASE)
# A whole PEM block with a body, not a label quoted in code or docs.
PRIVATE_KEY = re.compile(
    rb"-----BEGIN ([A-Z0-9-]+ )?PRIVATE KEY-----\r?\n(?:[A-Za-z0-9+/=]{16,}\r?\n){2,}"
    rb"[A-Za-z0-9+/=]*\r?\n?-----END ([A-Z0-9-]+ )?PRIVATE KEY-----"
)


# The only wasm the branch keeps: Flutter's CanvasKit, which the web app would otherwise download
# from a CDN, so an offline web build has it.
ALLOWED_WASM = ("cdn/www.gstatic.com/flutter-canvaskit/",)


class Problem(Exception):
    """What stops a command, for the one line it prints."""


class Crate(NamedTuple):
    name: str
    version: str
    checksum: str

    @property
    def dir(self) -> str:
        return f"{self.name}-{self.version}"

    @property
    def file(self) -> str:
        return f"{self.name}-{self.version}.crate"


class Repo(NamedTuple):
    url: str
    commit: str

    @property
    def name(self) -> str:
        return self.url.rstrip("/").rsplit("/", 1)[-1].removesuffix(".git")


class Lock(NamedTuple):
    crates: list[Crate]
    # Each repository at a revision, with the (name, version) Cargo.lock takes from it.
    repos: dict[Repo, list[tuple[str, str]]]
    # Each git source as Cargo.lock spells it, which is what a [source] entry has to replace.
    sources: dict[str, Repo]


class Tree(NamedTuple):
    """What a crates/ holds: each package directory's files with their SHA-256, and what is loose."""

    # Directory name -> file path below it -> SHA-256, `.cargo-checksum.json` included.
    dirs: dict[str, dict[str, str]]
    # Directory name -> its parsed `.cargo-checksum.json`.
    sums: dict[str, dict]
    # Directory name -> first line of its Cargo.toml, which names a git package's commit.
    heads: dict[str, str]
    # Files directly in crates/, which a vendor directory does not have.
    loose: list[str]
    # Directory name -> the files in it that are not text or images, as "path: kind".
    binaries: dict[str, list[str]]


class Scan(NamedTuple):
    """A whole commit: its crates/, and every path or file a scanner would refuse."""

    tree: Tree
    flagged: list[str]


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


def wanted_packages(lock: Lock) -> dict[str, tuple[str | None, Repo | None]]:
    """Directory name -> (the registry checksum or None, the repository or None)."""
    wanted: dict[str, tuple[str | None, Repo | None]] = {}
    entries = [(c.dir, (c.checksum, None)) for c in lock.crates]
    entries += [(f"{n}-{v}", (None, r)) for r, packages in lock.repos.items() for n, v in packages]
    for name, entry in entries:
        if name in wanted:
            raise Problem(f"{name} is taken from more than one source; a vendor directory has one")
        wanted[name] = entry
    return wanted


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def entropy(data: bytes) -> float:
    counts = collections.Counter(data)
    return -sum(v / len(data) * math.log2(v / len(data)) for v in counts.values()) if data else 0.0


def flagged_kind(body: bytes, name: str) -> str | None:
    """The kind of data the branch must not hold in a file's bytes, or None.

    Text and images pass. Archives, executables, keys, packs, wasm and any other binary do not.
    """
    head = body[:600]
    if len(head) >= 262 and head[257:262] == b"ustar":
        return "tar"
    for magic, kind in FLAG_MAGIC:
        if head.startswith(magic):
            return kind
    if KEY_EXT.search(name):
        return "key or keystore file"
    for magic, kind in ((b"\x00asm", "wasm"), (b"%PDF", "pdf"), (b"SQLite format 3", "sqlite")):
        if head.startswith(magic):
            return kind
    if any(head.startswith(m) for m in MEDIA) or head[4:8] == b"ftyp":
        return None
    try:
        body.decode("utf-8")
        return "private key in text" if PRIVATE_KEY.search(body) else None
    except UnicodeDecodeError:
        pass
    return "opaque high-entropy data" if entropy(body[:65536]) > 7.5 else "unrecognised binary"


class Adder:
    """Files into a Tree, whether they come from a directory or from a commit."""

    def __init__(self, tree: Tree) -> None:
        self.tree = tree

    def add(self, path: str, data: bytes) -> None:
        directory, _, rest = path.partition("/")
        if not rest:
            self.tree.loose.append(directory)
            return
        self.tree.dirs.setdefault(directory, {})[rest] = sha256(data)
        if rest != CHECKSUM and (kind := flagged_kind(data, rest.rsplit("/", 1)[-1])):
            self.tree.binaries.setdefault(directory, []).append(f"{rest}: {kind}")
        if rest == CHECKSUM:
            try:
                self.tree.sums[directory] = json.loads(data)
            except ValueError:
                self.tree.sums[directory] = {}
        elif rest == "Cargo.toml":
            self.tree.heads[directory] = data.split(b"\n", 1)[0].decode("utf-8", "replace")


def read_dir(crates: Path) -> Tree:
    tree = Tree({}, {}, {}, [], {})
    adder = Adder(tree)
    if crates.is_dir():
        for path in sorted(crates.rglob("*")):
            if path.is_file():
                adder.add(path.relative_to(crates).as_posix(), path.read_bytes())
    return tree


def blobs(checkout: Path, commit: str) -> Iterator[tuple[str, str, bytes]]:
    """Each file of a commit: its path, its mode and its bytes exactly as git stores them."""
    listing = subprocess.run(
        ["git", "-C", str(checkout), "ls-tree", "-r", "-z", commit], capture_output=True
    )
    if listing.returncode != 0:
        raise Problem(f"commit {commit} is not in {checkout}")
    reader = subprocess.Popen(
        ["git", "-C", str(checkout), "cat-file", "--batch"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
    )
    try:
        for entry in listing.stdout.split(b"\0"):
            if not entry:
                continue
            meta, _, raw = entry.partition(b"\t")
            mode, kind, object_id = meta.decode().split()
            if kind != "blob":
                continue
            reader.stdin.write(object_id.encode() + b"\n")
            reader.stdin.flush()
            _, _, size = reader.stdout.readline().split()
            body = reader.stdout.read(int(size))
            reader.stdout.read(1)
            yield raw.decode("utf-8", "surrogateescape"), mode, body
    finally:
        reader.stdin.close()
        reader.wait()


def scan_ref(checkout: Path, commit: str) -> Scan | None:
    """A commit's crates/ and everything in it a scanner would refuse; None when it is not here."""
    if subprocess.run(
        ["git", "-C", str(checkout), "cat-file", "-e", f"{commit}^{{commit}}"], capture_output=True
    ).returncode:
        return None
    tree = Tree({}, {}, {}, [], {})
    adder = Adder(tree)
    flagged = []
    for path, mode, body in blobs(checkout, commit):
        if any(part.lower() == ".git" for part in path.split("/")):
            flagged.append(f"{path}: a path below .git")
        if mode == "120000":
            flagged.append(f"{path}: a symbolic link")
        elif kind := flagged_kind(body, path.rsplit("/", 1)[-1]):
            if not (kind == "wasm" and path.startswith(ALLOWED_WASM)):
                flagged.append(f"{path}: {kind}")
        if path.startswith("crates/"):
            adder.add(path.removeprefix("crates/"), body)
    return Scan(tree, flagged)


def package_problems(
    tree: Tree, name: str, checksum: str | None, repo: Repo | None
) -> list[str]:
    """What keeps one directory from being the package Cargo.lock names, for a line each."""
    where = f"crates/{name}"
    if name not in tree.dirs:
        return [f"{where}: missing, Cargo.lock names it"]
    sums = tree.sums.get(name)
    if sums is None:
        return [f"{where}: no {CHECKSUM}"]
    found = []
    if sums.get("package") != checksum:
        found.append(f"{where}: package checksum {sums.get('package')}, Cargo.lock says {checksum}")
    if repo and f" at {repo.commit}" not in tree.heads.get(name, ""):
        found.append(f"{where}: not {repo.name} at {repo.commit}")
    found += [f"{where}/{line}; run refresh" for line in tree.binaries.get(name, [])]
    listed = sums.get("files", {})
    have = {p: d for p, d in tree.dirs[name].items() if p != CHECKSUM}
    for path in sorted(set(listed) - set(have)):
        found.append(f"{where}/{path}: listed in {CHECKSUM}, missing")
    for path in sorted(set(have) - set(listed)):
        found.append(f"{where}/{path}: not listed in {CHECKSUM}")
    for path in sorted(set(have) & set(listed)):
        if have[path] != listed[path]:
            found.append(f"{where}/{path}: SHA-256 {have[path]}, {CHECKSUM} says {listed[path]}")
    return found


def problems(lock: Lock, tree: Tree) -> list[str]:
    """One line for each thing keeping a crates/ from covering Cargo.lock, and nothing more."""
    found = []
    wanted = wanted_packages(lock)
    for name, (checksum, repo) in wanted.items():
        found += package_problems(tree, name, checksum, repo)
    for name in sorted(set(tree.dirs) - set(wanted)):
        found.append(f"crates/{name}: stale, Cargo.lock does not name it")
    for name in sorted(tree.loose):
        found.append(f"crates/{name}: a file; a vendor directory holds only package directories")
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

def archive_files(data: bytes, prefix: str) -> dict[str, bytes]:
    """The regular files of a .crate below its `<name>-<version>/` directory, by relative path."""
    files = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tar:
        for member in tar:
            if not member.isfile():
                continue
            if not member.name.startswith(prefix):
                raise Problem(f"{prefix.rstrip('/')}.crate holds {member.name}, outside {prefix}")
            files[member.name.removeprefix(prefix)] = tar.extractfile(member).read()
    return files


def keeps(path: str) -> bool:
    """Whether a package's file (by path below the package) is something compiling can read."""
    *dirs, name = path.split("/")
    if dirs and dirs[0].lower() in DROP_TOP:
        return False
    if any(d.lower() in DROP_ANY or d.startswith(".") for d in dirs):
        return False
    return not (name.startswith(".git") or name == ".cargo-ok")


def drop_missing_targets(text: str, present: set[str]) -> str:
    """The manifest without the [[test]], [[bench]] and [[example]] tables whose file is not here.

    Cargo refuses a manifest naming a target it cannot find, even in a dependency.
    """
    try:
        data = tomllib.loads(text)
    except tomllib.TOMLDecodeError:
        return text
    folders = {"test": "tests", "bench": "benches", "example": "examples"}
    index = dict.fromkeys(folders, -1)
    out, skipping = [], False
    for line in text.split("\n"):
        header = re.match(r"^\[\[(test|bench|example)\]\]\s*(#.*)?$", line)
        if header:
            kind = header[1]
            index[kind] += 1
            target = data.get(kind, [])[index[kind]]
            name = target.get("name", "")
            path = target.get("path") or None
            options = [path] if path else [f"{folders[kind]}/{name}.rs", f"{folders[kind]}/{name}/main.rs"]
            skipping = not any(Path(p).as_posix() in present for p in options if p)
        elif re.match(r"^\[", line):
            skipping = False
        if not skipping:
            out.append(line)
    return "\n".join(out)


def inline_bytes(kept: dict[str, bytes], files: dict[str, bytes], refused: set[str]) -> None:
    """Writes the bytes of each refused file into the `include_bytes!` that reads it.

    The array literal has the type `include_bytes!` gives, so a crate that embeds a DER file or a
    table compiles without the binary file being in the branch. A file that only tests read, or
    the big-endian copy of a table, is left out and its `include_bytes!` as it was.
    """

    def literal(base: str):
        def replace(found: re.Match[str]) -> str:
            target = posixpath.normpath(posixpath.join(base, found[2]))
            if target not in refused or BIG_ENDIAN.search(target) or TEST_ONLY.search(base + "/"):
                return found[0]
            body = files[target]
            array = "[" + ",".join(f"0x{b:02x}u8" if i == 0 else f"0x{b:02x}" for i, b in enumerate(body)) + "]"
            # `*include_bytes!(..)` is the array itself; plain `include_bytes!(..)` a reference to it.
            return array if found[1] else "&" + array

        return replace

    for path in [p for p in kept if p.endswith(".rs") and not TEST_ONLY.search(p)]:
        try:
            text = kept[path].decode("utf-8")
        except UnicodeDecodeError:
            continue
        changed = INCLUDE_BYTES.sub(literal(posixpath.dirname(path)), text)
        if changed != text:
            kept[path] = changed.encode("utf-8")


RING_OBJECT = re.compile(r"^pregenerated/.+\.o$")
RING_USE = """            let obj_srcs = asm_srcs
                .iter()
                .map(|src| obj_path(generated_dir, src.as_path()))
                .collect::<Vec<_>>();"""
RING_FN = """
/// The object file this crate carries as `<name>.o.hex`, written below `out_dir` for the linker.
fn object_from_text(generated_dir: &Path, out_dir: &Path, src: &Path) -> PathBuf {
    let text = fs::read_to_string(format!("{}.hex", obj_path(generated_dir, src).display())).unwrap();
    let digits: Vec<u8> = text.bytes().filter(|b| b.is_ascii_hexdigit()).collect();
    let bytes: Vec<u8> = digits
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    let path = obj_path(out_dir, src);
    fs::write(&path, bytes).unwrap();
    path
}
"""


def patch_ring(name: str, kept: dict[str, bytes], refused: dict[str, bytes]) -> None:
    """Keeps ring's prebuilt nasm objects (linked on x86 Windows) as hex, decoded by build.rs."""
    objects = sorted(p for p in refused if RING_OBJECT.match(p))
    if not objects:
        return
    build = kept["build.rs"].decode("utf-8")
    if build.count(RING_USE) != 1:
        raise Problem(f"{name}: build.rs no longer links its objects as crates.py expects")
    for path in objects:
        digits = refused.pop(path).hex()
        lines = [digits[i : i + 128] for i in range(0, len(digits), 128)]
        kept[f"{path}.hex"] = ("\n".join(lines) + "\n").encode("ascii")
    fixed = build.replace(RING_USE, RING_USE.replace("obj_path(generated_dir, src.as_path())", "object_from_text(generated_dir, out_dir, src.as_path())"))
    kept["build.rs"] = (fixed.rstrip("\n") + "\n" + RING_FN).encode("utf-8")


# Crates whose build links binary files that cannot be inlined, by crate name.
PATCHES = {"ring": patch_ring}


def include_closure(files: dict[str, bytes]) -> dict[str, bytes]:
    """The files compiling can read: `keeps`, and whatever a kept source file includes by path."""
    kept = {p: b for p, b in files.items() if keeps(p)}
    pending = [p for p in kept if p.endswith(".rs")]
    while pending:
        path = pending.pop()
        try:
            text = kept[path].decode("utf-8")
        except UnicodeDecodeError:
            continue
        for found in INCLUDE.finditer(text):
            target = posixpath.normpath(posixpath.join(posixpath.dirname(path), found[1] or found[2]))
            if target in files and target not in kept:
                kept[target] = files[target]
                if target.endswith(".rs"):
                    pending.append(target)
    return kept


def write_package(target: Path, everything: dict[str, bytes], checksum: str | None) -> None:
    """Writes a package directory with its .cargo-checksum.json, leaving out what a scanner refuses.

    The files map lists what was written, so cargo checks each file it reads against it.
    """
    files = include_closure(everything)
    seen: dict[str, str] = {}
    for path in files:
        if seen.setdefault(path.lower(), path) != path:
            raise Problem(f"{target.name} has {path} and {seen[path.lower()]}, one file on Windows")
        if any(RESERVED.match(part) for part in path.split("/")):
            raise Problem(f"{target.name}/{path} is a name Windows cannot create")
    kept, refused = {}, {}
    for path, body in files.items():
        name = path.rsplit("/", 1)[-1]
        if flagged_kind(body, name) == "private key in text":
            body = PEM_MARKER.sub(PEM_REPLACEMENT, body)
        if flagged_kind(body, name):
            refused[path] = body
        else:
            kept[path] = body
    if refused:
        inline_bytes(kept, files, set(refused))
        patch = PATCHES.get(target.name.rsplit("-", 1)[0])
        if patch:
            patch(target.name, kept, refused)
    if "Cargo.toml" in kept:
        text = kept["Cargo.toml"].decode("utf-8")
        fixed = drop_missing_targets(text, set(kept))
        if fixed != text:
            kept["Cargo.toml"] = fixed.encode("utf-8")
    partial = target.with_name(target.name + ".partial")
    shutil.rmtree(partial, ignore_errors=True)
    for path, body in kept.items():
        (partial / path).parent.mkdir(parents=True, exist_ok=True)
        (partial / path).write_bytes(body)
    listed = {path: sha256(body) for path, body in sorted(kept.items())}
    (partial / CHECKSUM).write_bytes(
        json.dumps({"files": listed, "package": checksum}, separators=(",", ":")).encode()
    )
    shutil.rmtree(target, ignore_errors=True)
    partial.rename(target)


def materialize_crate(crate: Crate, crates: Path) -> None:
    files = archive_files(fetch_crate(crate), f"{crate.dir}/")
    write_package(crates / crate.dir, files, crate.checksum)


def export_commit(repo: Repo, into: Path) -> Path:
    """The tree of exactly this commit below `into`, with no history: git checks the hash it fetched."""
    work = into / "fetch"
    work.mkdir()
    run_git("init", "-q", cwd=work)
    run_git("fetch", "-q", "--depth", "1", repo.url, repo.commit, cwd=work)
    if run_git("rev-parse", "FETCH_HEAD", cwd=work) != repo.commit:
        raise Problem(f"{repo.url} did not give commit {repo.commit}")
    probe = ["git", "-C", str(work), "cat-file", "-e", "FETCH_HEAD:.gitmodules"]
    if subprocess.run(probe, capture_output=True).returncode == 0:
        raise Problem(f"{repo.name} has submodules, which a source tree would leave out")
    tree = into / "tree"
    tree.mkdir()
    made = subprocess.Popen(
        [
            "git", "-C", str(work), "-c", "core.autocrlf=false", "-c", "core.eol=lf",
            "archive", "--format=tar", "FETCH_HEAD",
        ],  # fmt: skip
        stdout=subprocess.PIPE,
    )
    with tarfile.open(fileobj=made.stdout, mode="r|") as tar:
        tar.extractall(tree, filter="data")
    # git pads the archive to a whole record after its end marker; read it, or the pipe stays full
    # and git never exits (a Windows pipe holds 4 KiB).
    made.stdout.read()
    if made.wait():
        raise Problem(f"git archive of {repo.name} at {repo.commit} failed")
    shutil.rmtree(work, ignore_errors=True)
    return tree


def materialize_repo(repo: Repo, wanted: list[tuple[str, str]], crates: Path) -> None:
    """Puts each package Cargo.lock takes from a repository in a directory of its own in crates/."""
    with tempfile.TemporaryDirectory(prefix="ai6-git-") as scratch:
        tree = export_commit(repo, Path(scratch))
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

        packages = {d for d, m in manifests.items() if "package" in m}
        for name, version in wanted:
            found = []
            for directory in packages:
                manifest = manifests[directory]
                package = manifest["package"]
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
                raise Problem(f"{repo.name} at {repo.commit} has {len(found)} packages {name} {version}")
            directory, manifest, root_dir, root = found[0]
            label = f"{repo.name}/{directory.relative_to(tree).as_posix()}"
            written = normalized_manifest(manifest, root, directory, root_dir, label)
            nested = [d for d in packages if d != directory and directory in d.parents]
            files = {}
            for path in directory.rglob("*"):
                if path.is_file() and not any(n in path.parents for n in nested):
                    files[path.relative_to(directory).as_posix()] = path.read_bytes()
            lines = [f"# {label} at {repo.commit}, with what it inherits written out."]
            toml_table(written, [], lines)
            files["Cargo.toml"] = ("\n".join(lines) + "\n").encode("utf-8")
            # Git packages have no registry checksum.
            write_package(crates / f"{name}-{version}", files, None)


def sync(deps_dir: Path, lock: Lock) -> list[str]:
    """Makes deps_dir/crates cover the lock and hold nothing else; returns a line per change."""
    crates = deps_dir / "crates"
    wanted = wanted_packages(lock)
    changes = []
    crates.mkdir(parents=True, exist_ok=True)
    legacy = [
        p for p in crates.rglob("*") if p.is_file() and LEGACY_FILE.match(p.relative_to(crates).as_posix())
    ]
    for path in legacy:
        path.unlink()
    if legacy:
        changes.append(f"removed {len(legacy)} archives of an older layout")
    old = crates / "git"
    if old.is_dir() and not any(old.iterdir()):
        old.rmdir()
    for entry in sorted(crates.iterdir()):
        if entry.name.endswith(".partial") and entry.is_dir():
            shutil.rmtree(entry)
        elif not entry.is_dir() or (entry.name not in wanted and not (entry / CHECKSUM).is_file()):
            raise Problem(f"{entry} was not made by this script: is {crates} from a deps checkout?")
    tree = read_dir(crates)
    for name in sorted(set(tree.dirs) - set(wanted)):
        shutil.rmtree(crates / name)
        changes.append(f"removed {name}")
    stale = {n for n, (c, r) in wanted.items() if package_problems(tree, n, c, r)}

    def made(name: str) -> str:
        return f"{'replaced' if name in tree.dirs else 'added'} {name}"

    crate_jobs = [c for c in lock.crates if c.dir in stale]
    repo_jobs = [(r, p) for r, p in lock.repos.items() if any(f"{n}-{v}" in stale for n, v in p)]
    with ThreadPoolExecutor(max_workers=8) as pool:
        for future in [pool.submit(materialize_crate, c, crates) for c in crate_jobs] + [
            pool.submit(materialize_repo, r, p, crates) for r, p in repo_jobs
        ]:
            future.result()
    changes += [made(name) for name in sorted(stale)]
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



def source_entry(source: str) -> list[str]:
    """The keys naming a git source as Cargo.lock spells it: its url and one of rev, tag, branch."""
    parts = urllib.parse.urlsplit(source.removeprefix("git+"))
    lines = [f"git = {toml_value(f'{parts.scheme}://{parts.netloc}{parts.path}')}"]
    for key, value in urllib.parse.parse_qsl(parts.query):
        lines.append(f"{key} = {toml_value(value)}")
    return lines



def config_text(lock: Lock, crates: Path) -> str:
    """The config `cargo vendor` prints: every source replaced by the one vendor directory."""
    lines = [
        f"{CONFIG_MARK} Every source Cargo.lock names, from a directory.",
        "[net]",
        "offline = true",
        "",
        "[source.crates-io]",
        f'replace-with = "{SOURCE}"',
        "",
        f"[source.{SOURCE}]",
        f"directory = {toml_value(crates.as_posix())}",
    ]
    for source in sorted(lock.sources):
        name = source.partition("#")[0]
        lines += ["", f"[source.{toml_value(name)}]", *source_entry(source)]
        lines.append(f'replace-with = "{SOURCE}"')
    return "\n".join(lines) + "\n"


def use(deps_dir: Path, lock: Lock, lock_path: Path, out: Path | None, repo_config: bool) -> None:
    crates = (deps_dir / "crates").resolve()
    found = problems(lock, read_dir(crates))
    if found:
        raise Problem("; ".join(found[:5]) + f" ({len(found)} in all); run sync first")
    if out is None:
        out = Path(tempfile.gettempdir()) / f"ai6-crates-{sha256(lock_path.read_bytes())[:12]}"
    out = out.resolve()
    if out == ROOT or ROOT in out.parents:
        raise Problem(f"{out} is inside the repository; --out names a directory outside it")
    home = out / "cargo-home"
    config = home / "config.toml"
    if config.is_file() and not config.read_text(encoding="utf-8").startswith(CONFIG_MARK):
        raise Problem(f"{config} is not one this command wrote; it was left alone")
    home.mkdir(parents=True, exist_ok=True)
    config.write_bytes(config_text(lock, crates).encode("utf-8"))
    if repo_config:
        target = ROOT / ".cargo" / "config.toml"
        if target.exists() and not target.read_text(encoding="utf-8").startswith(CONFIG_MARK):
            raise Problem(f"{target} is not one this command wrote; it was left alone")
        target.parent.mkdir(exist_ok=True)
        target.write_bytes(config.read_bytes())
        print(f"wrote {target}; delete it to build from the network again")
    print(
        f"offline build ready: {len(lock.crates)} crates, {len(lock.repos)} git repositories: {home}"
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
    pinned = scan_ref(ROOT, line[1])
    if not committed and pinned is not None and not problems(lock, pinned.tree) and not pinned.flagged:
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
    scanned = scan_ref(ROOT, pin)
    found = problems(lock, scanned.tree) if scanned else ["the commit is not here"]
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
        "--out", type=Path, help="use: where to write the config (default: a new temporary directory)"
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
                f"ok: {len(lock.crates)} crates, {len(lock.repos)} git sources, "
                f"{len(changes)} changed"
            )
        elif args.command == "check":
            found = problems(lock, read_dir(args.deps_dir / "crates"))
            for line in found:
                print(line, file=sys.stderr)
            if found:
                return 1
            print(
                f"ok: {len(lock.crates)} crates, {len(lock.repos)} git sources "
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
