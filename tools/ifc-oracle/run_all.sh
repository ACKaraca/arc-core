#!/usr/bin/env bash
# Runs the arc-io-ifc tests with ARC_IFC_DUMP into a temporary directory, validates every file
# written and compares geometry. Outside the gate. Usage: run_all.sh [dump-dir]
set -euo pipefail
cd "$(dirname "$0")/../.."
dump="${1:-$(mktemp -d)}"
mkdir -p "$dump"
ARC_IFC_DUMP="$dump" cargo test -p arc-io-ifc --all-features --tests -q
echo "dumped: $(find "$dump" -name '*.ifc' | wc -l) files in $dump" >&2
status=0
python3 tools/ifc-oracle/validate.py "$dump" || status=1
for f in "$dump"/*.ifc; do
  python3 tools/ifc-oracle/geometry_compare.py "$f" || status=1
done
exit $status
