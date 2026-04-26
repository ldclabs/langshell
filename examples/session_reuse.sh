#!/usr/bin/env bash
set -euo pipefail

export LANGSHELL_SESSION_DIR="${LANGSHELL_SESSION_DIR:-$(mktemp -d)}"
cargo run -q -p langshell-cli --bin langshell -- run -e 'cache = {"k": 1}' --session-id s1 --json
cargo run -q -p langshell-cli --bin langshell -- run -e 'result = cache["k"] + 1' --session-id s1 --json