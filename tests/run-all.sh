#!/usr/bin/env bash
DIR="$(cd "$(dirname "$0")/.." && pwd)"; rc=0
for t in "$DIR"/tests/*_test.sh; do echo "== $(basename "$t") =="; bash "$t" || rc=1; done
exit $rc
