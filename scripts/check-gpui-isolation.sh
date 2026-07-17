#!/bin/sh
# The Zed-core-portability guarantee: only the `prdesk` app crate may depend
# on gpui. Fails if any domain crate grows a gpui dependency.
set -eu
cd "$(dirname "$0")/.."

dependents=$(cargo tree -i gpui --edges normal --prefix none 2>/dev/null \
    | awk '{print $1}' | sort -u | grep -v '^gpui' || true)

if [ "$dependents" != "prdesk" ]; then
    echo "gpui isolation violated. Crates depending on gpui:" >&2
    echo "$dependents" >&2
    echo "(only 'prdesk' is allowed)" >&2
    exit 1
fi
echo "ok: gpui is only reachable from 'prdesk'"
