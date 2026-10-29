#!/bin/sh
# Builds the Linux and Windows release binaries in containers, starts each
# on a machine with nothing else installed, and copies them to $OUT
# (default ${TMPDIR:-/tmp}/tribunal-docker-out) for a closer look.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
ctx=$(cd "$here/../.." && pwd)
out="${OUT:-${TMPDIR:-/tmp}/tribunal-docker-out}"
for target in linux-smoke windows-smoke; do
  echo "== $target"
  docker buildx build -f "$here/Dockerfile" --target "$target" --progress plain "$ctx"
done
rm -rf "$out"
docker buildx build -f "$here/Dockerfile" --target linux-bin --output "type=local,dest=$out/linux" "$ctx"
docker buildx build -f "$here/Dockerfile" --target windows-bin --output "type=local,dest=$out/windows" "$ctx"
ls -l "$out/linux" "$out/windows"
