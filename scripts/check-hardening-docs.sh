#!/usr/bin/env bash
set -euo pipefail

inventory=docs/plans/v0.1-hardening/documentation-inventory.md
ledger=docs/plans/v0.1-hardening/pre-hardening-removal-ledger.md

while IFS= read -r path; do
    grep -Fq "\`$path\`" "$inventory" || {
        printf 'documentation inventory is missing %s\n' "$path" >&2
        exit 1
    }
done < <(find docs -name '*.md' -type f | sort)

while IFS=: read -r file target; do
    target=${target%%#*}
    case "$target" in
        '' | /* | file://* | http://* | https://* | mailto:*) continue ;;
    esac
    test -e "$(dirname "$file")/$target" || {
        printf 'broken Markdown link in %s: %s\n' "$file" "$target" >&2
        exit 1
    }
done < <(
    while IFS= read -r file; do
        awk '/^```/{fenced=!fenced; next} !fenced' "$file" |
            grep -o -E '\]\([^ )#]+(#[^ )]+)?\)' |
            sed -E "s|^\\]\\(|$file:|; s/\\)$//" || true
    done < <(find docs -name '*.md' -type f | sort)
)

for id in CLEAN-{001..007}; do
    grep -Fq "$id" "$ledger" || {
        printf 'removal ledger is missing %s\n' "$id" >&2
        exit 1
    }
done

if grep -R -n -E '#\[deprecated|#\[(allow|expect)\(deprecated\)\]|#!\[allow\(deprecated\)\]' crates \
    --include='*.rs'; then
    printf 'deprecated production API or broad test allowance remains\n' >&2
    exit 1
fi

printf 'hardening documentation checks passed\n'
