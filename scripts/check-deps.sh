#!/usr/bin/env bash
set -euo pipefail

metadata_file=$(mktemp)
trap 'rm -f "$metadata_file"' EXIT

cargo metadata --format-version=1 --no-deps > "$metadata_file"

# Verify gestalt-core does not depend on any implementation crate.
CORE_DEPS=$(jq -r '.packages[]
  | select(.name == "gestalt-core")
  | [.dependencies[] | select(.kind == null and .path != null) | .name]
  | sort
  | .[]?' "$metadata_file")

if [ -n "$CORE_DEPS" ]; then
  printf 'ERROR: gestalt-core has path dependencies on implementation crates:\n%s\n' "$CORE_DEPS" >&2
  printf 'This violates ADR-001 (inverted crate dependency direction).\n' >&2
  exit 1
fi

printf 'OK: gestalt-core has no path dependencies on implementation crates.\n'

declare -A budgets=(
  ["gestalt-core"]=11
  ["gestalt-models"]=9
  ["gestalt-tools"]=14
  ["gestalt-context"]=6
  ["gestalt-policy"]=7
  ["gestalt-trace"]=9
  ["gestalt-harness"]=18
  ["gestalt-runtime"]=12
)

packages=(
  "gestalt-core"
  "gestalt-models"
  "gestalt-tools"
  "gestalt-context"
  "gestalt-policy"
  "gestalt-trace"
  "gestalt-harness"
  "gestalt-runtime"
)

failures=0

for package in "${packages[@]}"; do
  package_json=$(jq -c --arg name "$package" '.packages[] | select(.name == $name)' "$metadata_file")

  if [ -z "$package_json" ]; then
    printf 'ERROR: package %s missing from cargo metadata\n' "$package" >&2
    failures=1
    continue
  fi

  counted_count=$(jq -r '[.dependencies[] | select(.kind == null and .source != null and (.optional | not)) | .name] | sort | unique | length' <<<"$package_json")
  counted_csv=$(jq -r '[.dependencies[] | select(.kind == null and .source != null and (.optional | not)) | .name] | sort | unique | join(", ")' <<<"$package_json")
  optional_csv=$(jq -r '[.dependencies[] | select(.kind == null and .source != null and .optional) | .name] | sort | unique | join(", ")' <<<"$package_json")
  path_csv=$(jq -r '[.dependencies[] | select(.kind == null and .source == null) | .name] | sort | unique | join(", ")' <<<"$package_json")
  dev_csv=$(jq -r '[.dependencies[] | select(.kind == "dev") | .name] | sort | unique | join(", ")' <<<"$package_json")
  budget=${budgets[$package]}

  if [ "$counted_count" -gt "$budget" ]; then
    printf 'ERROR: %s exceeds dependency budget (%s > %s)\n' "$package" "$counted_count" "$budget" >&2
    printf '  counted default external deps: %s\n' "$counted_csv" >&2
    failures=1
  else
    printf 'OK: %s counted %s/%s default external deps\n' "$package" "$counted_count" "$budget"
    printf '  counted: %s\n' "${counted_csv:-none}"
  fi

  printf '  optional external: %s\n' "${optional_csv:-none}"
  printf '  path deps: %s\n' "${path_csv:-none}"
  printf '  dev deps: %s\n' "${dev_csv:-none}"
done

if [ "$failures" -ne 0 ]; then
  exit 1
fi
