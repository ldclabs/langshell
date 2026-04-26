#!/usr/bin/env bash
set -euo pipefail

cargo run -q -p langshell-cli --bin langshell -- validate -e 'open("/etc/passwd")' --json || true