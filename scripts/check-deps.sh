#!/usr/bin/env bash
set -euo pipefail

# Verify gestalt-core does not depend on any implementation crate
CORE_DEPS=$(cargo metadata --format-version=1 \
  | jq -r '.packages[]
    | select(.name == "gestalt-core")
    | .dependencies[]
    | select(.path != null)
    | .name')

if [ -n "$CORE_DEPS" ]; then
  echo "ERROR: gestalt-core has path dependencies on implementation crates:"
  echo "$CORE_DEPS"
  echo "This violates ADR-001 (inverted crate dependency direction)."
  exit 1
fi

echo "OK: gestalt-core has no path dependencies on implementation crates."
