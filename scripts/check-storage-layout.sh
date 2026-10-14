#!/usr/bin/env bash
# SC-02 / CI: fail if the LendingPool PoolStorage layout drifts from the committed baseline.
# Compares label, slot, offset and normalised type. Solidity AST ids change whenever the source
# changes, so they are ignored. Upgrades must append storage, never move or retype existing slots.
set -euo pipefail
cd "$(dirname "$0")/../contracts"

normalise() {
  python3 - "$1" <<'PY'
import json, re, sys
d = json.load(open(sys.argv[1]))
def norm(t):
    return re.sub(r"(t_\w+?\([^)]*\))\d+_storage", r"\1_storage", t)
def clean(x):
    if isinstance(x, dict):
        return {norm(k): clean(v) for k, v in x.items() if k not in ("astId", "contract")}
    if isinstance(x, list):
        return [clean(v) for v in x]
    if isinstance(x, str):
        return norm(x)
    return x
print(json.dumps(clean(d), indent=2, sort_keys=True))
PY
}

current=$(mktemp)
trap 'rm -f "$current"' EXIT
forge inspect StorageProbe storage-layout --json > "$current"

if ! diff <(normalise storage-layout.baseline.json) <(normalise "$current") > /dev/null; then
  echo "storage layout changed. Review it, then refresh the baseline on purpose:" >&2
  echo "  forge inspect StorageProbe storage-layout --json > contracts/storage-layout.baseline.json" >&2
  diff <(normalise storage-layout.baseline.json) <(normalise "$current") || true
  exit 1
fi
echo "storage layout matches baseline"
