#!/usr/bin/env bash
set -euo pipefail

tmpdir="$(mktemp -d)"
export LANGSHELL_SESSION_DIR="$tmpdir/sessions"
snapshot="$tmpdir/s2.snap"

cargo run -q -p langshell-cli --bin langshell -- run -e 'state = {"step": 1}' --session-id s2 --json
cargo run -q -p langshell-cli --bin langshell -- session snapshot s2 --out "$snapshot"
cargo run -q -p langshell-cli --bin langshell -- session restore --from "$snapshot" --session-id s2r
cargo run -q -p langshell-cli --bin langshell -- run -e 'result = state["step"]' --session-id s2r --json