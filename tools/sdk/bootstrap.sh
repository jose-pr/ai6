#!/bin/sh
# Installs what tools/sdk/sdk.lock names under one directory, verified, by version. Never touches
# the shell profile, PATH or the system, and never elevates. tools/sdk/README.md has the details.
#
# Usage: bootstrap.sh [--groups core,app] [--prefix DIR] [--check] [--print-system]
set -eu

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
lock="$here/sdk.lock"
groups=core,app
prefix=${AI6_SDK_HOME:-${HOME:?}/.ai6-sdk}
mode=install
missing=0
tmp_dir=
trap '[ -z "$tmp_dir" ] || rm -rf "$tmp_dir"' EXIT

die() {
  printf 'bootstrap: %s\n' "$*" >&2
  exit 1
}

while [ $# -gt 0 ]; do
  case $1 in
    --groups)
      [ $# -ge 2 ] || die "--groups needs a value"
      groups=$2
      shift 2
      ;;
    --prefix)
      [ $# -ge 2 ] || die "--prefix needs a value"
      prefix=$2
      shift 2
      ;;
    --check)
      mode=check
      shift
      ;;
    --print-system)
      mode=print-system
      shift
      ;;
    -h | --help)
      sed -n '2,5p' "$0"
      exit 0
      ;;
    *) die "unknown argument: $1" ;;
  esac
done

for group in $(printf '%s' "$groups" | tr ',' ' '); do
  case $group in
    core | app | api | docs | test) ;;
    *) die "unknown group: $group (core, app, api, docs, test)" ;;
  esac
done
case $prefix in
  *\'*) die "a prefix containing a single quote cannot be written to env.sh: $prefix" ;;
  /*) ;;
  *) prefix=$(pwd)/$prefix ;;
esac

case $(uname -s) in
  Linux)
    os=linux
    precache_flag=--linux
    ;;
  Darwin)
    os=macos
    precache_flag=--macos
    ;;
  *) die "use bootstrap.ps1 on Windows; no SDK is listed for $(uname -s)" ;;
esac
case $(uname -m) in
  x86_64 | amd64) arch=x64 ;;
  aarch64 | arm64) arch=arm64 ;;
  *) die "no SDK is listed for $(uname -m)" ;;
esac
platform=$os-$arch
case $platform in
  linux-x64) triple=x86_64-unknown-linux-gnu ;;
  linux-arm64) triple=aarch64-unknown-linux-gnu ;;
  macos-x64) triple=x86_64-apple-darwin ;;
  macos-arm64) triple=aarch64-apple-darwin ;;
esac

export RUSTUP_HOME="$prefix/rust/rustup"
export CARGO_HOME="$prefix/rust/cargo"
export FLUTTER_WINDOWING=true
rustup_bin=$CARGO_HOME/bin/rustup

wants() {
  case ",$groups," in
    *,"$1",*) return 0 ;;
    *) return 1 ;;
  esac
}

# Prints column $3 of the first line of group $1, item $2, for this platform or any.
lock_field() {
  awk -v g="$1" -v n="$2" -v p="$platform" -v c="$3" \
    '$1 == g && $2 == n && ($4 == p || $4 == "any") { print $c; exit }' "$lock"
}

lock_versions() {
  awk -v g="$1" -v n="$2" '$1 == g && $2 == n { print $3 }' "$lock"
}

report() {
  printf '%-32s %s\n' "$1" "$2"
}

need() {
  missing=1
  report "$1" "$2"
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{ print $1 }'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{ print $1 }'
  else
    die "neither sha256sum nor shasum is here, so nothing can be verified"
  fi
}

download() {
  if command -v curl >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$2" "$1"
  else
    die "neither curl nor wget is here"
  fi
}

# Sets $fetched to where a lock item's kept file belongs and, when one is there, hashes it: a copy
# that changed on disk is refused rather than trusted.
verify_kept() {
  fetch_name=$1
  fetch_version=$2
  fetch_sha=$3
  fetch_url=$4
  fetch_base=${fetch_url##*/}
  fetch_dir=$prefix/dl/$fetch_name-$fetch_version
  fetched=$fetch_dir/$fetch_base
  [ -e "$fetched" ] || return 0
  actual=$(sha256_of "$fetched")
  if [ "$actual" != "$fetch_sha" ]; then
    rm -f "$fetched"
    die "refused and removed $fetched
  sdk.lock says $fetch_sha
  the file was  $actual"
  fi
}

# Sets $fetched to the verified file for a lock item, downloading only when no copy is kept.
fetch_verified() {
  verify_kept "$@"
  [ ! -e "$fetched" ] || return 0
  report "$fetch_name $fetch_version" "fetching $fetch_url"
  mkdir -p "$fetch_dir"
  tmp_dir=$(mktemp -d "$prefix/tmp.XXXXXX")
  download "$fetch_url" "$tmp_dir/$fetch_base" || die "could not download $fetch_url"
  actual=$(sha256_of "$tmp_dir/$fetch_base")
  if [ "$actual" != "$fetch_sha" ]; then
    rm -rf "$tmp_dir"
    tmp_dir=
    die "refused $fetch_url
  sdk.lock says $fetch_sha
  the download  $actual"
  fi
  mv "$tmp_dir/$fetch_base" "$fetched"
  rm -rf "$tmp_dir"
  tmp_dir=
}

# Whether toolchain $1 is here with components $2 and targets $3. Asks only the prefix, so a run
# with nothing to do stays off the network.
toolchain_ready() {
  [ -x "$rustup_bin" ] || return 1
  "$rustup_bin" toolchain list 2>/dev/null | grep -q "^$1-$triple" || return 1
  for ready_component in $2; do
    "$rustup_bin" component list --toolchain "$1" --installed 2>/dev/null |
      grep -q "^$ready_component" || return 1
  done
  for ready_target in $3; do
    "$rustup_bin" target list --toolchain "$1" --installed 2>/dev/null |
      grep -qx "$ready_target" || return 1
  done
}

rustup_ready() {
  [ -x "$rustup_bin" ] &&
    [ "$("$rustup_bin" --version 2>/dev/null | awk '{ print $2; exit }')" = "$(lock_field core rustup-init 3)" ]
}

install_rustup() {
  rustup_version=$(lock_field core rustup-init 3)
  rustup_sha=$(lock_field core rustup-init 5)
  rustup_url=$(lock_field core rustup-init 6)
  verify_kept rustup-init "$rustup_version" "$rustup_sha" "$rustup_url"
  if rustup_ready; then
    report "rustup $rustup_version" ok
    return 0
  fi
  fetch_verified rustup-init "$rustup_version" "$rustup_sha" "$rustup_url"
  chmod +x "$fetched"
  RUSTUP_INIT_SKIP_PATH_CHECK=yes "$fetched" -y --no-modify-path --default-toolchain none \
    --profile minimal
  "$rustup_bin" set auto-self-update disable >/dev/null
  report "rustup $rustup_version" installed
}

# Installs toolchain $1 with components $2 and targets $3.
install_toolchain() {
  if toolchain_ready "$1" "$2" "$3"; then
    report "rust $1" ok
    return 0
  fi
  toolchain_args=
  for component in $2; do toolchain_args="$toolchain_args -c $component"; done
  for target in $3; do toolchain_args="$toolchain_args -t $target"; done
  # shellcheck disable=SC2086 # the names in the lock have no spaces, and each is its own argument
  "$rustup_bin" toolchain install "$1" --profile minimal --no-self-update $toolchain_args
  report "rust $1" installed
}

group_core() {
  components=$(lock_versions core rust-component | tr '\n' ' ')
  targets=$(lock_versions core rust-target | tr '\n' ' ')
  if [ "$mode" = check ]; then
    if rustup_ready; then
      report "rustup $(lock_field core rustup-init 3)" ok
    else
      need "rustup $(lock_field core rustup-init 3)" "missing, would fetch $(lock_field core rustup-init 6)"
    fi
    for version in $(lock_versions core rust); do
      if toolchain_ready "$version" "$components" "$targets"; then
        report "rust $version" ok
      else
        need "rust $version" "missing, rustup would fetch it with $components$targets"
      fi
    done
    return 0
  fi
  install_rustup
  for version in $(lock_versions core rust); do
    install_toolchain "$version" "$components" "$targets"
  done
  default=$(lock_versions core rust | head -n 1)
  if ! "$rustup_bin" default 2>/dev/null | grep -q "^$default-$triple"; then
    "$rustup_bin" default "$default-$triple" >/dev/null
  fi
}

group_api() {
  nightly=$(lock_field api rust 3)
  tool=$(lock_field api cargo-public-api 3)
  stable=$(lock_versions core rust | head -n 1)
  installed_tool=$("$CARGO_HOME/bin/cargo-public-api" --version 2>/dev/null | awk '{ print $2 }') || true
  if [ "$mode" = check ]; then
    if toolchain_ready "$nightly" "" ""; then
      report "rust $nightly" ok
    else
      need "rust $nightly" "missing, rustup would fetch it"
    fi
    if [ "$installed_tool" = "$tool" ]; then
      report "cargo-public-api $tool" ok
    else
      need "cargo-public-api $tool" "missing, cargo would compile it"
    fi
    return 0
  fi
  install_rustup
  install_toolchain "$stable" "" ""
  install_toolchain "$nightly" "" ""
  if [ "$installed_tool" = "$tool" ]; then
    report "cargo-public-api $tool" ok
  else
    "$rustup_bin" run "$stable" cargo install cargo-public-api --version "$tool" --locked
  fi
}

git_head_is() {
  [ -e "$1/.git" ] && [ "$(git -C "$1" rev-parse HEAD 2>/dev/null)" = "$2" ]
}

json_field() {
  sed -n "s/.*\"$2\": *\"\([^\"]*\)\".*/\1/p" "$1"
}

# A directory for a commit: a full hash would put Flutter's deepest files past Windows' path limit,
# and the two scripts lay the prefix out alike.
short() {
  printf '%s' "$1" | cut -c1-12
}

flutter_root() {
  printf '%s/flutter/%s' "$prefix" "$(short "$(lock_field app flutter 3)")"
}

deps_dir_of() {
  printf '%s/deps/%s' "$prefix" "$(short "$1")"
}

pub_cache_of() {
  printf '%s/pub-cache/%s' "$prefix" "$(short "$1")"
}

deps_ready() {
  git_head_is "$(deps_dir_of "$1")" "$1" && [ -d "$(pub_cache_of "$1")/hosted" ]
}

install_deps() {
  deps_commit=$1
  deps_url=${AI6_SDK_DEPS_URL:-$(lock_field app deps 6)}
  deps_dir=$(deps_dir_of "$deps_commit")
  if ! git_head_is "$deps_dir" "$deps_commit"; then
    rm -rf "$deps_dir.partial"
    mkdir -p "$deps_dir.partial"
    report "deps $deps_commit" "fetching from $deps_url"
    git -C "$deps_dir.partial" init -q
    git -C "$deps_dir.partial" config core.autocrlf false
    # By commit, so a later commit on the branch changes nothing here.
    git -C "$deps_dir.partial" fetch -q --depth 1 "$deps_url" "$deps_commit" ||
      die "could not fetch deps $deps_commit from $deps_url"
    git -C "$deps_dir.partial" checkout -q --detach FETCH_HEAD
    [ "$(git -C "$deps_dir.partial" rev-parse HEAD)" = "$deps_commit" ] ||
      die "deps from $deps_url is not $deps_commit"
    rm -rf "$deps_dir"
    mv "$deps_dir.partial" "$deps_dir"
  fi
  pub_cache=$(pub_cache_of "$deps_commit")
  if [ ! -d "$pub_cache/hosted" ]; then
    # A copy: pub writes into the cache it uses.
    mkdir -p "$prefix/pub-cache"
    rm -rf "$pub_cache.partial" "$pub_cache"
    cp -R "$deps_dir/pub" "$pub_cache.partial"
    # Pub takes a git package from its checkout once the checkout has .git/pub-packages, which
    # it fills in itself; git cannot carry a .git path, so deps has none.
    for checkout in "$pub_cache.partial"/git/*/; do
      [ -d "$checkout" ] || continue
      mkdir -p "$checkout.git" && : > "$checkout.git/pub-packages"
    done
    mv "$pub_cache.partial" "$pub_cache"
  fi
  report "deps $deps_commit" ok
}

install_flutter() {
  flutter_rev=$(lock_field app flutter 3)
  flutter_url=${AI6_SDK_FLUTTER_URL:-$(lock_field app flutter 6)}
  root=$(flutter_root)
  if ! git_head_is "$root" "$flutter_rev"; then
    rm -rf "$root.partial"
    mkdir -p "$prefix/flutter"
    report "flutter $flutter_rev" "cloning $flutter_url"
    # Flutter takes its version from the commits and tags, and needs a real checkout: a commit-only
    # clone gives it both and fetches just the trees and files of this revision.
    git -c core.autocrlf=false clone -q -c core.autocrlf=false --filter=tree:0 --no-checkout \
      "$flutter_url" "$root.partial" || die "could not clone $flutter_url"
    git -C "$root.partial" checkout -q --detach "$flutter_rev" ||
      die "$flutter_url has no commit $flutter_rev"
    rm -rf "$root"
    mv "$root.partial" "$root"
  fi
  if [ -n "${AI6_SDK_FLUTTER_URL:-}" ]; then
    export FLUTTER_GIT_URL="$AI6_SDK_FLUTTER_URL"
  fi
  export FLUTTER_ROOT="$root"
  PUB_CACHE=$(pub_cache_of "$(lock_field app deps 3)")
  export PUB_CACHE
  stamp=$root/bin/cache/.ai6-sdk-precache
  if [ ! -e "$stamp" ]; then
    report "flutter $(lock_field app flutter-version 3)" "fetching its Dart and engine for $os and web"
    # From its own directory: it hashes its engine version with a git command that reads the current
    # directory, and fails in one whose .git it cannot follow.
    (
      cd "$root"
      bin/flutter --version >/dev/null
      bin/flutter precache "$precache_flag" --web
    )
    verify_flutter "$root"
    : >"$stamp"
  else
    verify_flutter "$root"
  fi
  report "flutter $(lock_field app flutter-version 3)" ok
}

# What Flutter reports about itself, against what the lock expects of that commit.
verify_flutter() {
  version_file=$1/bin/cache/flutter.version.json
  [ -e "$version_file" ] || die "flutter wrote no $version_file"
  got=$(json_field "$version_file" frameworkVersion)
  want=$(lock_field app flutter-version 3)
  [ "$got" = "$want" ] || die "flutter reports version $got, sdk.lock says $want"
  got=$(cat "$1/bin/cache/engine.stamp")
  want=$(lock_field app flutter-engine 3)
  [ "$got" = "$want" ] || die "flutter's engine is $got, sdk.lock says $want"
  got=$(json_field "$version_file" dartSdkVersion)
  want=$(lock_field app dart 3)
  case $got in
    *"$want"*) ;;
    *) die "flutter's Dart is $got, sdk.lock says $want" ;;
  esac
}

group_app() {
  commit=$(lock_field app deps 3)
  root=$(flutter_root)
  if [ "$mode" = check ]; then
    if deps_ready "$commit"; then
      report "deps $commit" ok
    else
      need "deps $commit" "missing, would fetch from ${AI6_SDK_DEPS_URL:-$(lock_field app deps 6)}"
    fi
    if git_head_is "$root" "$(lock_field app flutter 3)" && [ -e "$root/bin/cache/.ai6-sdk-precache" ]; then
      report "flutter $(lock_field app flutter-version 3)" ok
    else
      need "flutter $(lock_field app flutter-version 3)" \
        "missing, would clone ${AI6_SDK_FLUTTER_URL:-$(lock_field app flutter 6)} at $(lock_field app flutter 3), then its engine for $os and web"
    fi
    return 0
  fi
  command -v git >/dev/null 2>&1 || die "git is needed for the app group; see --print-system"
  install_deps "$commit"
  install_flutter
}

group_docs() {
  version=$(lock_field docs mkdocs 3)
  venv=$prefix/docs/mkdocs-$version
  requirements=$here/docs-requirements.txt
  if [ -x "$venv/bin/mkdocs" ] && cmp -s "$requirements" "$venv/requirements.txt"; then
    report "docs mkdocs $version" ok
    return 0
  fi
  if [ "$mode" = check ]; then
    need "docs mkdocs $version" "missing, pip would fetch what docs-requirements.txt lists, hash-checked"
    return 0
  fi
  command -v python3 >/dev/null 2>&1 || die "python3 is needed for the docs group; see --print-system"
  rm -rf "$venv"
  mkdir -p "$prefix/docs"
  python3 -m venv "$venv"
  "$venv/bin/python" -m pip install --quiet --disable-pip-version-check --require-hashes -r "$requirements"
  cp "$requirements" "$venv/requirements.txt"
  report "docs mkdocs $version" installed
}

group_test() {
  version=$(lock_field test node 3)
  sha=$(lock_field test node 5)
  url=$(lock_field test node 6)
  node_dir=$prefix/node/$version
  image="$(lock_field test postgres 6 | sed 's|^docker://||'):$(lock_field test postgres 3)@sha256:$(lock_field test postgres 5)"
  report "postgres image" "$image, left to your container engine"
  if [ -x "$node_dir/bin/node" ]; then
    verify_kept node "$version" "$sha" "$url"
    report "node $version" ok
    return 0
  fi
  if [ "$mode" = check ]; then
    need "node $version" "missing, would fetch $url"
    return 0
  fi
  fetch_verified node "$version" "$sha" "$url"
  rm -rf "$node_dir.partial"
  mkdir -p "$node_dir.partial"
  tar -xJf "$fetched" -C "$node_dir.partial" --strip-components=1
  mv "$node_dir.partial" "$node_dir"
  report "node $version" installed
}

system_item_found() {
  case $1 in
    pkg:*) pkg-config --exists "${1#pkg:}" 2>/dev/null ;;
    xcodebuild) xcodebuild -version >/dev/null 2>&1 ;;
    docker-or-podman) command -v docker >/dev/null 2>&1 || command -v podman >/dev/null 2>&1 ;;
    *) command -v "$1" >/dev/null 2>&1 ;;
  esac
}

check_system() {
  system_missing=0
  system_names=$(awk -v p="$platform" '$1 == "system" && ($4 == p || $4 == "any") { print $3 ":" $2 }' "$lock")
  for system_item in $system_names; do
    system_group=${system_item%%:*}
    system_name=${system_item#*:}
    wants "$system_group" || continue
    if system_item_found "$system_name"; then
      report "system $system_name" found
    else
      system_missing=1
      report "system $system_name" "MISSING; see --print-system"
    fi
  done
  if [ "$system_missing" = 1 ]; then
    [ "$mode" != check ] || missing=1
    printf 'bootstrap: some of the system group is missing. Nothing here installs it:\n' >&2
    print_system >&2
  fi
}

print_system() {
  case $os in
    linux)
      printf '# Fedora, RHEL\n'
      printf 'sudo dnf install -y git curl gcc clang cmake ninja-build pkgconf-pkg-config gtk3-devel xz-devel libstdc++-devel unzip xz python3 openssl\n'
      printf '# Debian, Ubuntu\n'
      printf 'sudo apt-get install -y git curl build-essential clang cmake ninja-build pkg-config libgtk-3-dev liblzma-dev unzip xz-utils python3 python3-venv openssl\n'
      ;;
    macos)
      printf '# Xcode, from the App Store or developer.apple.com, then:\n'
      printf 'sudo xcode-select --switch /Applications/Xcode.app\n'
      printf 'sudo xcodebuild -license accept\n'
      ;;
  esac
  printf '# Docker or Podman, for the test group, as its own installer says\n'
}

# Quotes $1 for a POSIX shell or PowerShell, both of which take single quotes literally.
quoted() {
  printf "'%s'" "$1"
}

env_var() {
  printf 'export %s=%s\n' "$1" "$(quoted "$2")" >>"$env_sh.partial"
  printf '$env:%s = %s\n' "$1" "$(quoted "$2")" >>"$env_ps1.partial"
}

add_path() {
  paths="$1:$paths"
}

write_env() {
  deps_commit=$(lock_field app deps 3)
  root=$(flutter_root)
  paths=
  env_sh=$prefix/env.sh
  env_ps1=$prefix/env.ps1
  {
    printf '# Written by tools/sdk/bootstrap.sh. Source it: . %s\n' "$env_sh"
    printf 'export AI6_SDK_HOME=%s\n' "$(quoted "$prefix")"
    printf 'export FLUTTER_WINDOWING=true\n'
  } >"$env_sh.partial"
  {
    printf '# Written by tools/sdk/bootstrap.sh. Source it: . %s\n' "$env_ps1"
    printf '$env:AI6_SDK_HOME = %s\n' "$(quoted "$prefix")"
    printf '$env:FLUTTER_WINDOWING = '"'"'true'"'"'\n'
  } >"$env_ps1.partial"
  if [ -x "$rustup_bin" ]; then
    env_var RUSTUP_HOME "$RUSTUP_HOME"
    env_var CARGO_HOME "$CARGO_HOME"
    add_path "$CARGO_HOME/bin"
  fi
  if [ -e "$root/.git" ]; then
    env_var FLUTTER_ROOT "$root"
    add_path "$root/bin/cache/dart-sdk/bin"
    add_path "$root/bin"
    if [ -n "${AI6_SDK_FLUTTER_URL:-}" ]; then
      env_var FLUTTER_GIT_URL "$AI6_SDK_FLUTTER_URL"
    fi
  fi
  if [ -d "$(pub_cache_of "$deps_commit")" ]; then
    env_var PUB_CACHE "$(pub_cache_of "$deps_commit")"
    env_var AI6_SDK_DEPS "$(deps_dir_of "$deps_commit")"
  fi
  if [ -x "$prefix/node/$(lock_field test node 3)/bin/node" ]; then
    add_path "$prefix/node/$(lock_field test node 3)/bin"
  fi
  if [ -x "$prefix/docs/mkdocs-$(lock_field docs mkdocs 3)/bin/mkdocs" ]; then
    add_path "$prefix/docs/mkdocs-$(lock_field docs mkdocs 3)/bin"
  fi
  printf 'export PATH=%s"$PATH"\n' "$(quoted "$paths")" >>"$env_sh.partial"
  printf '$env:PATH = %s + $env:PATH\n' "$(quoted "$paths")" >>"$env_ps1.partial"
  mv "$env_sh.partial" "$env_sh"
  mv "$env_ps1.partial" "$env_ps1"
}

if [ "$mode" = print-system ]; then
  print_system
  exit 0
fi

[ "$mode" = check ] || mkdir -p "$prefix/dl"
report prefix "$prefix"
report platform "$platform"
! wants core || group_core
! wants app || group_app
! wants api || group_api
! wants docs || group_docs
! wants test || group_test
check_system
if [ "$mode" = check ]; then
  [ "$missing" = 0 ] || exit 1
  exit 0
fi
write_env
report env "$prefix/env.sh, $prefix/env.ps1"
