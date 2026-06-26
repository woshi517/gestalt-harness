#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$script_dir/.." && pwd)
# Keep the Linux release binary under the current measured ceiling.
threshold_bytes=$((30 * 1024 * 1024))

cargo build --release --locked --manifest-path "$repo_root/crates/gestalt-cli/Cargo.toml" --bin gestalt

binary="$repo_root/target/release/gestalt"

if [ ! -f "$binary" ]; then
  printf 'ERROR: expected release binary at %s\n' "$binary" >&2
  exit 1
fi

os_name=$(uname -s)
case "$os_name" in
  Linux)
    size_bytes=$(stat -c '%s' "$binary")
    ;;
  Darwin)
    size_bytes=$(stat -f '%z' "$binary")
    ;;
  *)
    printf 'ERROR: unsupported operating system for binary-size audit: %s\n' "$os_name" >&2
    exit 1
    ;;
esac

host_triple=$(
  rustc -vV | while IFS= read -r line; do
    case "$line" in
      host:*)
        printf '%s\n' "${line#host: }"
        break
        ;;
    esac
  done
)

size_mib_whole=$((size_bytes / 1048576))
size_mib_fraction=$((((size_bytes % 1048576) * 100) / 1048576))
threshold_mib_whole=$((threshold_bytes / 1048576))

printf 'binary=%s\n' "$binary"
printf 'os=%s\n' "$os_name"
printf 'target=%s\n' "$host_triple"
printf 'size_bytes=%s\n' "$size_bytes"
printf 'size_mib=%d.%02d\n' "$size_mib_whole" "$size_mib_fraction"
printf 'threshold_bytes=%s\n' "$threshold_bytes"
printf 'threshold_mib=%d\n' "$threshold_mib_whole"

if [ "$os_name" = "Linux" ] && [ "$size_bytes" -gt "$threshold_bytes" ]; then
  printf 'ERROR: release binary exceeds Linux threshold (%s > %s bytes)\n' "$size_bytes" "$threshold_bytes" >&2
  exit 1
fi

printf 'OK: binary-size audit completed\n'
