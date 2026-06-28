#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$script_dir/.." && pwd)
tmp_root=$(mktemp -d)
trap 'rm -rf "$tmp_root"' EXIT

install_root="$tmp_root/install-root"
home_root="$tmp_root/home"
host_home="${HOME:-$tmp_root/home}"
# Reuse the runner cargo cache, but keep HOME isolated so the install is hermetic.
cargo_home="${CARGO_HOME:-$host_home/.cargo}"

mkdir -p "$install_root" "$home_root/.config"

export CARGO_HOME="$cargo_home"
export HOME="$home_root"
export XDG_CONFIG_HOME="$home_root/.config"
# CI runners may not have a rustup default toolchain configured, so pin stable here.
export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}"

cargo install \
  --locked \
  --force \
  --offline \
  --path "$repo_root/crates/gestalt-cli" \
  --root "$install_root"

cargo install \
  --locked \
  --force \
  --offline \
  --path "$repo_root/crates/gestalt-tui" \
  --root "$install_root"

binary="$install_root/bin/gestalt"
tui_binary="$install_root/bin/gestalt-tui"
fixture_workspace="$repo_root/tests/fixtures/workspaces/minimal"

if [ ! -x "$binary" ]; then
  printf 'ERROR: installed binary missing or not executable: %s\n' "$binary" >&2
  exit 1
fi

if [ ! -x "$tui_binary" ]; then
  printf 'ERROR: installed binary missing or not executable: %s\n' "$tui_binary" >&2
  exit 1
fi

"$binary" --help >/dev/null
"$binary" --workspace "$fixture_workspace" config validate >/dev/null
"$tui_binary" --help >/dev/null
GESTALT_TUI_BIN=true "$binary" >/dev/null

printf 'OK: isolated install produced %s and %s and validated %s\n' "$binary" "$tui_binary" "$fixture_workspace"
