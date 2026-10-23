#!/bin/bash
# Publishes a list of tracks to a fresh FAT32 MBR disk image and reports what
# the export cost at each stage. The image stands in for a DJ stick: same
# filesystem, same per-file flush behaviour, none of the USB link's slowness.
#
# Usage: bench-fat32.sh <list-of-paths.txt> [size_gb]
set -euo pipefail

list="${1:?usage: bench-fat32.sh <list-of-paths.txt> [size_gb]}"
size_gb="${2:-8}"
[ -f "$list" ] || { echo "no such list: $list" >&2; exit 2; }

repo="$(cd "$(dirname "$0")/.." && pwd)"
image="$(mktemp -d)/bench.dmg"
volume="/Volumes/BENCH"
attached=0

cleanup() {
  if [ "$attached" = 1 ]; then
    hdiutil detach "$volume" -force >/dev/null 2>&1 || true
  fi
  rm -rf "$(dirname "$image")"
}
trap cleanup EXIT

echo "== creating ${size_gb}G FAT32 MBR image at $image"
hdiutil create -size "${size_gb}g" -fs "MS-DOS FAT32" -layout MBRSPUD \
  -volname BENCH "$image" >/dev/null

echo "== attaching"
hdiutil attach "$image" >/dev/null
attached=1
[ -d "$volume" ] || { echo "image did not mount at $volume" >&2; exit 3; }

echo "== export_cost over $(wc -l < "$list" | tr -d ' ') files"
cd "$repo"
cargo run --release -p rbl-export --example export_cost -- "$list" "$volume/out"

echo "== stick_check"
cargo run --release -p rbl-pdb --example stick_check -- "$volume/out"
