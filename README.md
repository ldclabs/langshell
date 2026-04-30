# LangShell

> Stateful, capability-scoped, sandboxed code execution for AI agents.

**[English](./README.md) | [中文](./README_CN.md)**

LangShell is a secure execution layer for AI agents. It lets a host run agent-authored Python or TypeScript code inside persistent, capability-scoped sessions, while keeping all interaction with files, networks, databases, and other external systems behind explicit host-registered functions.

The repository is in active development. Local session snapshots and internal data formats are allowed to change without migration support until the project declares a stable release. The current development baseline uses CBOR snapshots, AST-based static validation, and schema-described capabilities.

## What Works Today

- `langshell-core` defines shared contracts for sessions, capabilities, diagnostics, metrics, errors, and runtime backends.
- `langshell-monty` executes Python-subset code with persistent Monty sessions, top-level await, AST-based validation, result capture, external call auditing, and CBOR snapshots.
- `langshell-deno` executes TypeScript code through Deno/V8 with the same runtime trait, persistent globals, AST-based validation, typed CBOR snapshots, and async capability dispatch.
- `langshell-tools` registers discovery tools plus host-configurable file and HTTP capability helpers.
- `langshell` provides a backend-neutral Rust SDK builder for limits, mounts, allowlists, sync or async capabilities, and host-selected runtimes.
- `langshell-cli` provides `run`, `validate`, `repl`, `daemon`, `session`, and `tools` commands with stable JSON output.
- End-to-end scripts and SDK coverage live under [examples/README.md](./examples/README.md), backend tests, and SDK tests.

[AGENTS.md](AGENTS.md) is the product and engineering contract. [SKILL.md](SKILL.md) describes how an AI agent should use LangShell safely.

## Why LangShell

Traditional agent execution paths often fall between two poor choices:

- Tool calling is safe but fragmented. Complex work becomes many round trips, repeated context, brittle recovery, and more schema surface.
- A normal shell is expressive but overpowered. It has broad ambient access, weak structured output, fragile parsing, and no natural capability boundary.

LangShell is the middle layer:

```text
AI tokens -> sandboxed code -> mediated capabilities -> structured result -> resumable state
```

It is designed for three groups at once:

- AI agents write code for loops, branching, caching, retries, concurrency, and data transformation.
- Agent framework developers embed a stable runtime, register tools, enforce limits, and collect audit records.
- Platform and security owners keep the default policy closed and force every side effect through a named capability.

## Core Model

- **Code is the interface**: agents express multi-step logic as code instead of many tiny tool calls.
- **Session is the state unit**: variables, functions, globals, capabilities, limits, and snapshots belong to a session.
- **Capabilities replace ambient permissions**: filesystem, network, database, and business APIs are only reachable through registered functions.
- **Validation happens before execution**: Python and TypeScript validation uses AST checks for imports, dangerous globals, reflection escape patterns, and unknown capability-like calls.
- **Snapshots are development-versioned**: current snapshots are CBOR v2. Old local snapshots are not migrated.

## Implemented Capabilities

- Stateful Python-subset execution with Monty.
- Stateful TypeScript execution with Deno/V8.
- Top-level await and async capability calls.
- Validate / dry-run flows that catch syntax, permission, feature, and tool-availability problems before side effects.
- Capability discovery through `list_tools`, `describe_tool`, and `current_policy`.
- Structured `RunResult` output with `result`, stdout, stderr, diagnostics, external call records, metrics, and stable error codes.
- Result capture priority: global `result`, then final expression when supported, then stdout fallback.
- Resource controls for wall-clock timeout, output size, memory, stack depth, and external call count.
- CBOR snapshots for session restore. Deno snapshots include tagged values for `bigint`, `Uint8Array`, `Map`, `Set`, and `Date`.
- JSON-RPC daemon over Unix sockets for session, run, snapshot, restore, and tool operations.

## Current Constraints

- Monty is a Python subset, not CPython. Unsupported standard-library modules, third-party packages, subprocesses, raw sockets, and reflection escapes are blocked or unavailable.
- The TypeScript backend is available, but Deno/V8 runtime lifecycle is kept behind the `LanguageRuntime` trait and a dedicated worker.
- Built-in file tools only work when a host configures authorized virtual mounts.
- Built-in HTTP helpers enforce allowlists and schemas, but the default build does not ship live network transport. Hosts should register their own `fetch_text` or `fetch_json` capability for real HTTP access.
- The CLI daemon currently supports `unix://` listeners.
- Development data is disposable: local session files and snapshots may be invalidated by code changes.

## Architecture

```text
Agent / Host App
    |
    +-- CLI
    +-- JSON-RPC Daemon
    +-- Rust SDK
            |
            v
      langshell SDK
            |
     +------+----------------+
     |                       |
     v                       v
langshell-tools        langshell-core
                              |
                              v
                       LanguageRuntime trait
                              |
              +---------------+---------------+
              |                               |
              v                               v
       langshell-monty                 langshell-deno
              |                               |
              v                               v
           Monty VM                         Deno/V8
```

## Crates

| Crate             | Role                                                                                                    |
| ----------------- | ------------------------------------------------------------------------------------------------------- |
| `langshell-core`  | Shared contracts for sessions, runs, capabilities, diagnostics, metrics, snapshots, and runtime traits. |
| `langshell-monty` | Python runtime implementation backed by Monty.                                                          |
| `langshell-deno`  | TypeScript runtime implementation backed by Deno/V8.                                                    |
| `langshell-tools` | Discovery, file, and HTTP capability helpers.                                                           |
| `langshell`       | Public Rust SDK for composing runtimes and registering capabilities.                                    |
| `langshell-cli`   | CLI binary and JSON-RPC daemon.                                                                         |

## Repository Layout

```text
langshell/
├── crates/
│   ├── langshell/
│   ├── langshell-cli/
│   ├── langshell-core/
│   ├── langshell-deno/
│   ├── langshell-monty/
│   └── langshell-tools/
├── docs/
├── examples/
├── AGENTS.md
├── SKILL.md
└── README.md
```

## Python Example

```python
import json

async def main():
    items = await fetch_json("https://api.example.com/items")
    selected = [item for item in items if item.get("score", 0) >= 0.8]
    await write_text("/workspace/selected.json", json.dumps(selected))
    return {"selected": len(selected), "total": len(items)}

result = await main()
print(json.dumps(result))
```

## CLI Examples

```bash
cargo run -q -p langshell-cli --bin langshell -- run -e 'result = sum(range(10))' --json
cargo run -q -p langshell-cli --bin langshell -- validate -e 'open("/etc/passwd")' --json
cargo run -q -p langshell-cli --bin langshell -- session list
cargo run -q -p langshell-cli --bin langshell -- daemon --listen unix:///tmp/langshell.sock
```

## Rust SDK Example

```rust
use langshell::{LangShell, SideEffect};
use langshell_monty::MontyRuntime;
use serde_json::{Value, json};

let shell = LangShell::builder()
    .runtime(MontyRuntime::new)
    .register_async(
        "fetch_json",
        "Fetch JSON from an approved source.",
        SideEffect::Network,
        json!({"type": "array", "prefixItems": [{"type": "string"}], "minItems": 1, "maxItems": 1}),
        json!({"type": "object"}),
        |ctx| async move {
            let url = ctx.args.first().and_then(Value::as_str).unwrap_or_default();
            Ok(json!({"url": url, "ok": true}))
        },
    )?
    .build()?;
```

## JSON-RPC Request Shape

The daemon speaks line-delimited JSON-RPC 2.0 over a Unix socket.

```json
{
  "jsonrpc": "2.0",
  "id": "req-001",
  "method": "session.run",
  "params": {
    "session_id": "agent-123",
    "language": "python",
    "code": "result = sum(range(10))",
    "return_snapshot": true
  }
}
```

## Development

### Requirements

- Rust stable toolchain with Edition 2024 support.
- Git submodules.
- macOS, Linux, or Windows.

### Setup

```bash
git clone --recurse-submodules <repo-url>
cd langshell
```

If the repository was cloned without submodules:

```bash
git submodule update --init --recursive
```

### Checks

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```

### End-to-End Scripts

```bash
bash examples/cli_single.sh
bash examples/session_reuse.sh
bash examples/validate_denied.sh
bash examples/snapshot_restore.sh
cargo run -q -p langshell-monty --example sdk_async_fanout
```

The CLI persists development session snapshots under `LANGSHELL_SESSION_DIR` when set, or under the platform temporary directory. These files are not treated as stable storage.

## Near-Term Work

- Durable snapshot store.
- Session fork / diff / reset.
- Richer generated stubs and tool descriptions.
- Transport-backed HTTP helpers and more capability modules.
- Broader security tests for path escape, tool storms, snapshot corruption, and resource exhaustion.
- Windows named pipe daemon transport.

## Documentation

- [AGENTS.md](AGENTS.md): product direction, runtime contracts, data structures, snapshot format, error codes, and test matrix.
- [SKILL.md](SKILL.md): safe agent-facing usage guidance.
- [examples/README.md](./examples/README.md): runnable CLI and SDK examples.

## License

Copyright © LDC Labs

Licensed under the Apache License, Version 2.0.