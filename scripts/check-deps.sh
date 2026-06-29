#!/usr/bin/env bash
set -euo pipefail

metadata_file=$(mktemp)
trap 'rm -f "$metadata_file"' EXIT

cargo metadata --format-version=1 --no-deps > "$metadata_file"

expected_packages=$'gestalt-app\ngestalt-cli\ngestalt-core\ngestalt-runtime\ngestalt-tui'
actual_packages=$(jq -r '.packages[].name' "$metadata_file" | sort)

if [ "$actual_packages" != "$expected_packages" ]; then
  printf 'ERROR: workspace package set is incorrect\n' >&2
  printf 'Expected:\n%s\n' "$expected_packages" >&2
  printf 'Actual:\n%s\n' "$actual_packages" >&2
  exit 1
fi

printf 'OK: workspace contains exactly the five final packages.\n'

assert_path_deps_exact() {
  local package=$1
  local expected=$2
  local actual

  actual=$(jq -r --arg name "$package" '
    .packages[]
    | select(.name == $name)
    | [.dependencies[] | select(.kind == null and .path != null) | .name]
    | sort
    | join("\n")
  ' "$metadata_file")

  if [ "$actual" != "$expected" ]; then
    printf 'ERROR: %s path dependency set mismatch\n' "$package" >&2
    printf 'Expected:\n%s\n' "${expected:-<none>}" >&2
    printf 'Actual:\n%s\n' "${actual:-<none>}" >&2
    exit 1
  fi

  printf 'OK: %s path dependencies match the final matrix.\n' "$package"
}

assert_path_deps_exact "gestalt-core" ""
assert_path_deps_exact "gestalt-runtime" "gestalt-core"
assert_path_deps_exact "gestalt-app" $'gestalt-core\ngestalt-runtime'
assert_path_deps_exact "gestalt-cli" $'gestalt-app\ngestalt-core\ngestalt-runtime'
assert_path_deps_exact "gestalt-tui" $'gestalt-app\ngestalt-core\ngestalt-runtime'

minimal_cli_tree=$(cargo tree -p gestalt-cli --no-default-features --edges normal --prefix none)
if grep -Eq '^(ratatui|crossterm) v' <<<"$minimal_cli_tree"; then
  printf 'ERROR: minimal CLI includes terminal UI dependencies\n' >&2
  exit 1
fi

printf 'OK: minimal CLI excludes terminal UI dependencies.\n'

if [ -d crates/gestalt-runtime/src/legacy ]; then
  printf 'ERROR: runtime legacy source directory still exists\n' >&2
  exit 1
fi

if rg -n '#\[path\s*=\s*"legacy/|mod legacy_|crate::legacy_|pub use legacy_' \
  crates/gestalt-runtime/src >/dev/null; then
  printf 'ERROR: runtime contains a legacy module mount or reference\n' >&2
  exit 1
fi

printf 'OK: runtime source layout contains no legacy mounts.\n'

minimal_runtime_tree=$(cargo tree -p gestalt-runtime --no-default-features --edges normal --prefix none)
runtime_deny='^(encoding_rs|eventsource-stream|pulldown-cmark|regex|reqwest|serde_yaml|similar|tokio-stream|toml|walkdir) v'
if grep -Eq "$runtime_deny" <<<"$minimal_runtime_tree"; then
  printf 'ERROR: minimal runtime includes an optional integration dependency\n' >&2
  grep -E "$runtime_deny" <<<"$minimal_runtime_tree" >&2
  exit 1
fi

printf 'OK: minimal runtime excludes optional integration dependencies.\n'

if rg -n 'extern crate .* as gestalt_' crates/gestalt-runtime/src crates/gestalt-runtime/tests crates/gestalt-app/src crates/gestalt-app/tests crates/gestalt-cli/src crates/gestalt-cli/tests crates/gestalt-tui/src >/dev/null; then
  printf 'ERROR: compatibility aliases leaked across crate boundaries\n' >&2
  exit 1
fi

printf 'OK: no crate exports compatibility aliases.\n'
