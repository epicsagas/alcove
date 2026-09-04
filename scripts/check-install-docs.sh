#!/usr/bin/env bash
# Guard against install-docs drift (issue #50): the only installer assets ever
# published are the ones dist generates from dist-workspace.toml
# (alcove-installer.sh / alcove-installer.ps1). Docs must not point at
# hand-maintained scripts, and must not advertise an installer whose target
# family is absent from dist-workspace.toml.
set -uo pipefail

cd "$(dirname "$0")/.."

targets="$(sed -n '/^targets = \[/,/^\]/p' dist-workspace.toml)"
fail=0

has_sh()  { echo "$targets" | grep -qE '(apple-darwin|unknown-linux)'; }
has_ps1() { echo "$targets" | grep -qE 'pc-windows'; }

for f in README.md docs/README.*.md; do
    if grep -nE 'download/(install\.sh|install\.ps1)' "$f"; then
        echo "ERROR: $f references a hand-maintained installer (scripts/install.*) that is never published as a release asset" >&2
        fail=1
    fi
    if grep -qE 'download/alcove-installer\.ps1' "$f" && ! has_ps1; then
        echo "ERROR: $f advertises alcove-installer.ps1 but dist-workspace.toml has no pc-windows target" >&2
        fail=1
    fi
    if grep -qE 'download/alcove-installer\.sh' "$f" && ! has_sh; then
        echo "ERROR: $f advertises alcove-installer.sh but dist-workspace.toml has no darwin/linux target" >&2
        fail=1
    fi
done

if [ -e scripts/install.sh ] || [ -e scripts/install.ps1 ]; then
    echo "ERROR: scripts/install.{sh,ps1} must not exist — dist generates the published installers; hand-maintained copies drift (see issue #50)" >&2
    fail=1
fi

if [ "$fail" -eq 0 ]; then
    echo "install-docs check: OK"
fi
exit "$fail"
