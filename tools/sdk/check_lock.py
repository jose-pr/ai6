#!/usr/bin/env python3
"""Checks tools/sdk/sdk.lock against itself and against what the repository builds with.

Usage: check_lock.py [--lock FILE] [--pubspec-lock FILE] [--deps-ref REF] [--online]

Exits 0 when every check passes and 1 with one line per failure. --lock checks another copy of the
lock, and --pubspec-lock another copy of the app's pubspec.lock against the deps commit. --deps-ref is where to read the build containers' defaults (default origin/deps); a ref
without them is reported and skipped. --online also compares each download's hash with what its
upstream publishes, the only way to notice a well-formed hash that was changed.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import urllib.request
from pathlib import Path
from typing import NamedTuple

import crates

ROOT = Path(__file__).resolve().parents[2]
GROUPS = {"core", "app", "api", "docs", "test", "system"}
PLATFORMS = {"windows-x64", "windows-arm64", "linux-x64", "linux-arm64", "macos-arm64", "macos-x64"}
# A name that means "whatever is newest" is not a pin.
FLOATING = re.compile(r"^(stable|beta|nightly|latest|lts|main|master|head|dev)$", re.IGNORECASE)
HEX64 = re.compile(r"^[0-9a-f]{64}$")
HEX40 = re.compile(r"^[0-9a-f]{40}$")
PUBSPEC_LOCK = ROOT / "clients/outer-office/pubspec.lock"
MAX_BYTES = 20 * 1024


class Item(NamedTuple):
    line: int
    group: str
    name: str
    version: str
    platform: str
    sha256: str
    url: str


class Package(NamedTuple):
    name: str
    version: str
    source: str
    url: str
    path: str
    resolved_ref: str


def locked_packages(text: str) -> list[Package]:
    """The packages of a pubspec.lock, read by indentation: the file pub writes has one shape."""
    packages = []
    current: dict[str, str] = {}
    for line in text.splitlines():
        entry = re.match(r"^  ([A-Za-z0-9_]+):\s*$", line)
        field = re.match(r'^ {4,6}(source|version|url|path|resolved-ref): "?([^"]*)"?\s*$', line)
        if entry:
            if current:
                packages.append(Package(**current))
            current = {"name": entry[1], "version": "", "source": "", "url": "", "path": "", "resolved_ref": ""}
        elif field and current:
            current[field[1].replace("-", "_")] = field[2]
        elif not line.startswith(" ") and current:
            packages.append(Package(**current))
            current = {}
    if current:
        packages.append(Package(**current))
    return packages


class Checker:
    def __init__(self, lock: Path, pubspec_lock: Path = PUBSPEC_LOCK) -> None:
        self.lock = lock
        self.pubspec_lock = pubspec_lock
        self.errors: list[str] = []
        self.items: list[Item] = []
        self.notes: list[str] = []
        self.trees: dict[str, set[str]] = {}

    def error(self, message: str, line: int | None = None) -> None:
        where = f"{self.lock.name}:{line}: " if line else f"{self.lock.name}: "
        self.errors.append(where + message)

    def find(self, group: str, name: str) -> list[Item]:
        return [i for i in self.items if i.group == group and i.name == name]

    def one(self, group: str, name: str) -> Item | None:
        found = self.find(group, name)
        if len(found) != 1:
            self.error(f"expected one line for {group} {name}, found {len(found)}")
            return None
        return found[0]

    def parse(self) -> None:
        raw = self.lock.read_bytes()
        if len(raw) > MAX_BYTES:
            self.error(f"{len(raw)} bytes, over the {MAX_BYTES} a text lock should need")
        if b"\r" in raw:
            self.error("has CR line endings")
        seen: dict[tuple[str, str, str, str], int] = {}
        for number, text in enumerate(raw.decode("utf-8").split("\n"), 1):
            text = text.strip()
            if not text or text.startswith("#"):
                continue
            fields = text.split()
            if len(fields) != 6:
                self.error(f"expected 6 fields, found {len(fields)}", number)
                continue
            item = Item(number, *fields)
            self.check_item(item)
            key = (item.group, item.name, item.version, item.platform)
            if key in seen:
                self.error(f"same item as line {seen[key]}", number)
            seen[key] = number
            self.items.append(item)

    def check_item(self, item: Item) -> None:
        n = item.line
        if item.group not in GROUPS:
            self.error(f"unknown group {item.group}", n)
        if item.platform != "any" and item.platform not in PLATFORMS:
            self.error(f"unknown platform {item.platform}", n)
        if item.sha256 != "-" and not HEX64.match(item.sha256):
            self.error(f"hash of {item.name} is not 64 hex characters: {item.sha256}", n)
        if item.url != "-" and not re.match(r"^(https|docker)://\S+$", item.url):
            self.error(f"url of {item.name} is not https or docker: {item.url}", n)
        if FLOATING.match(item.version):
            self.error(f"{item.name} floats: {item.version}", n)
        if item.group == "system":
            if item.sha256 != "-" or item.url != "-":
                self.error(f"system item {item.name} has a hash or a url", n)
            if item.version not in GROUPS - {"system"}:
                self.error(f"system item {item.name} names no group: {item.version}", n)
        elif item.url.startswith("https://") and not item.url.endswith(".git") and item.sha256 == "-":
            self.error(f"download {item.name} has no hash", n)

    def check_platforms(self) -> None:
        for group, name in (("core", "rustup-init"), ("test", "node")):
            have = {i.platform for i in self.find(group, name)}
            missing = PLATFORMS - have
            if missing:
                self.error(f"{group} {name} is missing {', '.join(sorted(missing))}")

    def check_rust(self) -> None:
        versions = [i.version for i in self.find("core", "rust")]
        for item in self.find("core", "rust"):
            if not re.match(r"^\d+\.\d+\.\d+$", item.version):
                self.error(f"rust is not an exact version: {item.version}", item.line)
        manifest = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
        found = re.search(r'^rust-version\s*=\s*"([^"]+)"', manifest, re.MULTILINE)
        if not found:
            self.error("Cargo.toml has no rust-version")
        elif not any(v == found[1] or v.startswith(found[1] + ".") for v in versions):
            self.error(f"Cargo.toml's rust-version {found[1]} is not listed in {versions}")

    def check_nightly(self) -> None:
        item = self.one("api", "rust")
        tool = self.one("api", "cargo-public-api")
        if item:
            for path in (".github/workflows/test.yml", "tools/public-api.ps1"):
                text = (ROOT / path).read_text(encoding="utf-8")
                named = set(re.findall(r"nightly-\d{4}-\d{2}-\d{2}", text))
                if named != {item.version}:
                    self.error(f"{path} names {sorted(named)}, the lock {item.version}", item.line)
        if tool:
            text = (ROOT / ".github/workflows/test.yml").read_text(encoding="utf-8")
            found = re.search(r"cargo-public-api --version (\S+)", text)
            if not found or found[1] != tool.version:
                self.error(f"test.yml does not install cargo-public-api {tool.version}", tool.line)

    def check_flutter(self) -> None:
        item = self.one("app", "flutter")
        deps = self.one("app", "deps")
        if not item or not deps:
            return
        if not HEX40.match(item.version):
            self.error(f"flutter is not a commit: {item.version}", item.line)
        if not HEX40.match(deps.version):
            self.error(f"deps is not a commit: {deps.version}", deps.line)
        app = ROOT / "clients/outer-office"
        cdn = json.loads((app / "tool/cdn_files.json").read_text(encoding="utf-8"))
        engines = cdn.get("engine_revisions")
        if not isinstance(engines, list) or item.version not in engines:
            self.error(
                f"tool/cdn_files.json lists Flutter engines {engines}, "
                f"not the lock's {item.version}",
                item.line,
            )
        metadata = re.search(r'revision: "([0-9a-f]{40})"', (app / ".metadata").read_text("utf-8"))
        if not metadata or metadata[1] != item.version:
            self.error(f"clients/outer-office/.metadata does not name {item.version}", item.line)
        readme = self.git_show(deps.version, "README.md")
        if readme is None:
            self.error(
                f"deps commit {deps.version} is not here: "
                f"git fetch --depth 1 origin {deps.version}",
                deps.line,
            )
            return
        named = re.search(r"Built for Flutter engine `([0-9a-f]{40})`", readme)
        if not named:
            self.error(f"the README of deps at {deps.version} names no Flutter engine", deps.line)
        elif named[1] != item.version:
            self.error(f"deps at {deps.version} names Flutter {named[1]}, the lock {item.version}", item.line)
        self.check_pub(deps)

    def check_pub(self, deps: Item) -> None:
        """Every package pubspec.lock names is in deps' pub cache, or the offline build stops there.

        A package from the Flutter SDK comes with it, and a path package is the checkout's. A git
        package is pub's checkout of its repository at the locked revision, with no clone beside it
        (no pack file belongs on the branch); the bootstrap gives the checkout the `.git/pub-packages`
        an offline `pub get` reads, which git cannot carry.
        """
        packages = locked_packages(self.pubspec_lock.read_text(encoding="utf-8"))
        if not packages:
            self.error(f"{self.pubspec_lock.name} names no package", deps.line)
        for package in packages:
            if package.source == "hosted":
                host = package.url.removeprefix("https://").removeprefix("http://").rstrip("/")
                where = f"pub/hosted/{host}/{package.name}-{package.version}"
                found = self.tree_holds(deps.version, where)
            elif package.source == "git":
                repo = package.url.rstrip("/").rsplit("/", 1)[-1].removesuffix(".git")
                root = f"pub/git/{repo}-{package.resolved_ref}"
                where = f"{root}/{package.path}/pubspec.yaml".replace("//", "/")
                found = self.tree_holds(deps.version, where)
            else:
                continue
            if not found:
                self.error(
                    f"{self.pubspec_lock.name} names {package.name} {package.version}, "
                    f"but deps at {deps.version} has no {where}; refresh the deps branch",
                    deps.line,
                )

    def tree_holds(self, ref: str, path: str) -> bool:
        """Whether a commit holds `path`; a `*` matches the last part's end."""
        parent, _, name = path.rpartition("/")
        if parent not in self.trees:
            done = subprocess.run(
                ["git", "-C", str(ROOT), "ls-tree", "--name-only", f"{ref}:{parent}"],
                capture_output=True,
            )
            self.trees[parent] = set(done.stdout.decode("utf-8").split()) if done.returncode == 0 else set()
        if name.endswith("*"):
            return any(entry.startswith(name[:-1]) for entry in self.trees[parent])
        return name in self.trees[parent]

    def check_crates(self) -> None:
        """The deps commit the lock names holds what Cargo.lock names, and nothing else."""
        deps = self.find("app", "deps")
        if len(deps) != 1:
            return
        tree = crates.read_ref(ROOT, deps[0].version)
        if tree is None:
            self.error(f"deps at {deps[0].version} has no crates/, or is not here", deps[0].line)
            return
        for problem in crates.problems(crates.read_lock(ROOT / "Cargo.lock"), tree):
            self.error(
                f"deps at {deps[0].version}: {problem}; python3 tools/sdk/crates.py refresh DEPS_DIR",
                deps[0].line,
            )

    def check_docs(self) -> None:
        requirements = (ROOT / "tools/sdk/docs-requirements.txt").read_text(encoding="utf-8")
        pinned = dict(re.findall(r"^([A-Za-z0-9._-]+)==(\S+)", requirements, re.MULTILINE))
        for item in self.find("docs", "mkdocs") + self.find("docs", "mkdocs-material") + self.find(
            "docs", "pymdown-extensions"
        ):
            if pinned.get(item.name) != item.version:
                self.error(
                    f"docs-requirements.txt has {item.name} {pinned.get(item.name)}, "
                    f"the lock {item.version}",
                    item.line,
                )
        python = self.one("docs", "python")
        text = (ROOT / ".github/workflows/docs.yml").read_text(encoding="utf-8")
        found = re.search(r'python-version:\s*"([^"]+)"', text)
        if python and (not found or found[1] != python.version):
            self.error(f"docs.yml does not use Python {python.version}", python.line)

    def check_postgres(self) -> None:
        item = self.one("test", "postgres")
        if not item:
            return
        text = (ROOT / ".github/workflows/test.yml").read_text(encoding="utf-8")
        wanted = f"postgres:{item.version}@sha256:{item.sha256}"
        named = set(re.findall(r"postgres:\S+", text))
        named = {n for n in named if re.match(r"postgres:\d", n)}
        if named != {wanted}:
            self.error(f"test.yml runs {sorted(named)}, the lock {wanted}", item.line)

    def check_containers(self, ref: str) -> None:
        text = self.git_show(ref, "containers/Dockerfile.linux")
        if text is None:
            self.notes.append(f"no containers/Dockerfile.linux at {ref}: their defaults not checked")
            return
        windows = self.git_show(ref, "containers/Dockerfile.windows") or ""
        flutter = self.one("app", "flutter")
        deps = self.one("app", "deps")
        rust = self.find("core", "rust")
        if not flutter or not deps or not rust:
            return
        wanted = {
            "FLUTTER_REF": flutter.version,
            "DEPS_REF": deps.version,
            "RUST_VERSION": rust[0].version,
        }
        for platform, arg in (("linux-x64", "RUSTUP_SHA256_X64"), ("linux-arm64", "RUSTUP_SHA256_ARM64")):
            for item in self.find("core", "rustup-init"):
                if item.platform == platform:
                    wanted[arg] = item.sha256
        for name, value in wanted.items():
            found = re.search(rf"^ARG {name}=(\S+)$", text, re.MULTILINE)
            if not found or found[1] != value:
                self.error(f"Dockerfile.linux at {ref} defaults {name} to "
                           f"{found[1] if found else None}, the lock {value}")
        for item in self.find("core", "rustup-init"):
            if item.platform == "windows-x64":
                wanted["RUSTUP_SHA256"] = item.sha256
        for name in ("FLUTTER_REF", "DEPS_REF", "RUST_VERSION", "RUSTUP_SHA256"):
            found = re.search(rf"^ARG {name}=(\S+)$", windows, re.MULTILINE)
            if not found or found[1] != wanted[name]:
                self.error(f"Dockerfile.windows at {ref} defaults {name} to "
                           f"{found[1] if found else None}, the lock {wanted[name]}")

    def check_online(self) -> None:
        node_sums: dict[str, dict[str, str]] = {}
        for item in self.items:
            if item.name == "rustup-init":
                expected = fetch(item.url + ".sha256").split()[0]
            elif item.name == "node":
                folder = item.url.rsplit("/", 1)[0]
                if folder not in node_sums:
                    rows = fetch(folder + "/SHASUMS256.txt").splitlines()
                    node_sums[folder] = {r.split()[1]: r.split()[0] for r in rows}
                expected = node_sums[folder].get(item.url.rsplit("/", 1)[1], "")
            elif item.name == "llvm":
                expected = llvm_digest(item)
            else:
                continue
            if expected != item.sha256:
                self.error(f"{item.name} {item.platform}: upstream says {expected}", item.line)

    def git_show(self, ref: str, path: str) -> str | None:
        done = subprocess.run(
            ["git", "-C", str(ROOT), "show", f"{ref}:{path}"], capture_output=True
        )
        return done.stdout.decode("utf-8") if done.returncode == 0 else None


def fetch(url: str) -> str:
    request = urllib.request.Request(url, headers={"User-Agent": "ai6-check-lock"})
    with urllib.request.urlopen(request, timeout=60) as response:
        return response.read().decode("utf-8")


def llvm_digest(item: Item) -> str:
    tag = f"llvmorg-{item.version}"
    release = json.loads(fetch(f"https://api.github.com/repos/llvm/llvm-project/releases/tags/{tag}"))
    name = item.url.rsplit("/", 1)[1]
    for asset in release["assets"]:
        if asset["name"] == name:
            return str(asset.get("digest", "")).removeprefix("sha256:")
    return ""


def main() -> int:
    parser = argparse.ArgumentParser(description="Check tools/sdk/sdk.lock.")
    parser.add_argument("--lock", type=Path, default=ROOT / "tools/sdk/sdk.lock")
    parser.add_argument("--pubspec-lock", type=Path, default=PUBSPEC_LOCK)
    parser.add_argument("--deps-ref", default="origin/deps")
    parser.add_argument("--online", action="store_true")
    args = parser.parse_args()

    checker = Checker(args.lock, args.pubspec_lock)
    checker.parse()
    checker.check_platforms()
    checker.check_rust()
    checker.check_nightly()
    checker.check_flutter()
    checker.check_crates()
    checker.check_docs()
    checker.check_postgres()
    checker.check_containers(args.deps_ref)
    if args.online:
        checker.check_online()

    for note in checker.notes:
        print(f"note: {note}")
    for error in checker.errors:
        print(error, file=sys.stderr)
    if checker.errors:
        return 1
    print(f"ok: {len(checker.items)} items in {args.lock.name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
