#!/usr/bin/env bash
set -euo pipefail

cargo run -q -p langshell-cli --bin langshell -- run -e 'result = sum(range(10))' --json