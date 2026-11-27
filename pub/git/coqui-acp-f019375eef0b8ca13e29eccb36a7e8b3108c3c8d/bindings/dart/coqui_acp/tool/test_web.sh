#!/bin/sh
# Runs the secret-prompt tests in a browser (needs Chrome or Chromium: set CHROME_EXECUTABLE if it is not on the path).
# A browser cannot read the repository, so the vectors are copied beside the tests for the run and removed after it.
cd "$(dirname "$0")/.." || exit 2
cp ../../../vectors/secret.json test/secret_vectors.json || exit 2
trap 'rm -f test/secret_vectors.json' EXIT
dart test -p chrome test/secret_test.dart "$@"
