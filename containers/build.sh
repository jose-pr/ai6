#!/bin/sh
# Builds an app builder image with the commit a FLUTTER_REF resolves to in its label. A Dockerfile
# cannot set a label from what a build finds out, so the ref is resolved first and built by hash;
# that also keeps a moved branch from being answered from an old cached layer.
#
# Usage: [FLUTTER_REF=ref] [FLUTTER_GIT_URL=url] [ENGINE=podman] build.sh [engine build arguments]
set -eu

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
file=${DOCKERFILE:-$here/Dockerfile.linux}
default=$(sed -n 's/^ARG FLUTTER_REF=//p' "$file")
ref=${FLUTTER_REF:-$default}
url=${FLUTTER_GIT_URL:-$(sed -n 's/^ARG FLUTTER_GIT_URL=//p' "$file")}
engine=${ENGINE:-$(command -v podman >/dev/null 2>&1 && echo podman || echo docker)}

case $ref in
  *[!0-9a-f]* | "") commit= ;;
  *) [ "${#ref}" -eq 40 ] && commit=$ref || commit= ;;
esac
if [ -z "$commit" ]; then
  # An annotated tag lists its commit on the ^{} line, which sorts after the tag's own.
  commit=$(git ls-remote "$url" "refs/heads/$ref" "refs/tags/$ref" "refs/tags/$ref^{}" |
    tail -n 1 | cut -f1)
fi
if [ -z "$commit" ]; then
  echo "build.sh: FLUTTER_REF $ref is not a tag or branch of $url" >&2
  exit 1
fi
echo "build.sh: FLUTTER_REF $ref is commit $commit"
exec "$engine" build -f "$file" \
  --build-arg "FLUTTER_REF=$commit" \
  --build-arg "FLUTTER_GIT_URL=$url" \
  --label "ai6.flutter.commit=$commit" \
  "$@" "$here"
