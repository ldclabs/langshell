# 🖥️ LangShell

> Stateful, capability-scoped, sandboxed code execution for AI agents.

**[English](./README.md) | [中文](./README_CN.md)**

LangShell is a secure execution layer for AI agents. Its goal is to let an agent produce a piece of Python code that can be validated, resumed, and audited to complete complex work, instead of decomposing everything into a large number of fragile tool calls.

The project is implemented in Rust. The MVP uses Pydantic Monty as the Python-subset execution engine. The core idea behind LangShell is simple: treat code as the interface, sessions as the unit of state, and host-registered capabilities as the only entry points to the outside world.

## Current Status

This repository contains a working MVP of the core LangShell flow, not just crate scaffolding.

- `langshell-core` defines the stable data contracts for sessions, tools, diagnostics, errors, and snapshots.
- `langshell-monty` runs Python-subset code in persistent Monty sessions, supports validation, captures `result` and final-expression values, and records external calls.
- `langshell-tools` registers discovery tools plus opt-in file and HTTP capability helpers for hosts.
- `langshell` exposes a Rust SDK builder for mounts, allowlists, and custom sync or async capabilities.
- `langshell-cli` provides `run`, `validate`, `repl`, `daemon`, `session`, and `tools` commands with stable JSON output.
- End-to-end scripts and SDK coverage live under [examples/README.md](./examples/README.md) and `crates/langshell/tests`.

[AGENTS.md](AGENTS.md) remains the source of truth for product requirements and engineering contracts, and [SKILL.md](SKILL.md) describes how an AI agent should use LangShell safely.

## Why LangShell

Traditional agent execution paths usually fall into one of two extremes:

- Tool calling is too fragmented. Complex logic requires many round trips, costs more, and is hard to recover when something fails.
- A normal shell has too much privilege, weak state handling, and brittle output parsing. It is not a good place to run untrusted LLM-generated code.

LangShell is intended to provide a middle layer:

```text
AI tokens -> Python code -> safe execution -> structured result -> resumable state
```

It aims to serve three groups at once:

- AI agents: use familiar Python to express loops, branching, caching, retries, concurrency, and data transformation.
- Agent framework developers: embed execution through a stable protocol, register tools, enforce limits, and collect audit data.
- Platform and security owners: keep the system zero-permission by default and force all side effects through explicit capability boundaries.

## Design Principles

- Code is the interface: for the agent, the main interface is code rather than an ever-growing collection of tool schemas.
- Session is the unit: state, limits, auditing, snapshots, and lifecycle management all center on the session.
- Capabilities over permissions: nothing is allowed by default, and all external capabilities must be explicitly registered by the host.
- Every side effect is mediated: file, network, database, and other side effects must pass through host-defined capabilities.
- Errors are for agents: errors must be stable, structured, and useful for automatic repair and retry.

## Implemented MVP Scope

The current MVP provides:

- Stateful execution of a Python subset.
- Top-level await and async capability calls.
- Validate and dry-run modes that catch syntax, type, permission, and tool-availability issues without causing side effects.
- A capability registry so the host can expose controlled external functions.
- Capability discovery interfaces such as `list_tools`, `describe_tool`, and `current_policy`.
- Structured results, stdout and stderr capture, diagnostics, and stable error codes.
- Result capture priority of global `result`, then last expression, then stdout fallback.
- Limits for timeout, cancellation, output size, memory, and external call counts.
- Snapshot and restore for resumability and approval-boundary pauses.
- A Unix-socket JSON-RPC daemon path for session and tool operations.

The MVP also includes host-side helpers for controlled file and HTTP capability wiring, including `read_text`, `write_text`, `list_dir`, `fetch_text`, and `fetch_json`.

## Current Limitations

- The executable backend is Python-only today; TypeScript and Deno remain future work.
- File tools are only available when the host configures authorized mounts through the SDK builder.
- The built-in HTTP helpers enforce allowlists and capability shape, but do not ship a real network transport in the default build. Hosts should register their own `fetch_text` or `fetch_json` handlers for live HTTP access.
- The CLI daemon currently supports `unix://` listeners only.

## Architecture Overview

```text
Agent / Host App
	|
	+-- CLI
	+-- JSON-RPC Daemon
	+-- Rust SDK
	        |
	        v
	  langshell-core
	        |
	 +------+-------+
	 |              |
	 v              v
langshell-monty  langshell-tools
	 |
	 v
	 Monty VM
```

Responsibilities are split along these boundaries:

- `langshell-core`: core abstractions, including the stable contracts for sessions, policy, registry, snapshots, and diagnostics.
- `langshell-monty`: the MVP execution backend that encapsulates all Monty-specific integration.
- `langshell-tools`: built-in capability modules such as file and HTTP tools.
- `langshell-cli`: the developer-facing command-line entry point, intended to host commands such as run, validate, repl, daemon, session, and tools.
- `langshell`: the public Rust SDK for hosts to integrate the runtime, register capabilities, and initiate execution.

## Crates

| Crate             | Role                                                                                                            |
| ----------------- | --------------------------------------------------------------------------------------------------------------- |
| `langshell-core`  | Stable Rust and JSON-facing contracts for sessions, capabilities, diagnostics, metrics, and snapshots.          |
| `langshell-monty` | Monty-backed runtime implementation with persistent sessions, validation, result capture, and snapshot support. |
| `langshell-tools` | Built-in discovery tools and host-configurable file and HTTP capability helpers.                                |
| `langshell`       | Public Rust SDK for building runtimes, configuring policy, and registering sync or async capabilities.          |
| `langshell-cli`   | CLI binary and line-delimited JSON-RPC daemon for running code and inspecting sessions.                         |

## Repository Layout

```text
langshell/
├── monty/                  # upstream execution engine submodule
├── deno/                   # future TypeScript / Deno backend submodule
├── crates/
│   ├── langshell/
│   ├── langshell-cli/
│   ├── langshell-core/
│   ├── langshell-monty/
│   └── langshell-tools/
├── docs/
├── AGENTS.md
├── SKILL.md
└── README.md
```

The crate layout mirrors the engineering contract in [AGENTS.md](AGENTS.md) while mapping cleanly onto the code that ships in this MVP.

## Interface Examples

The following examples correspond to code paths that exist in this repository today.

### Agent-Side Python

This is the shape of code an agent can run once a host has registered the required capabilities:

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

### CLI Commands Available Today

```bash
cargo run -q -p langshell-cli --bin langshell -- run -e 'result = sum(range(10))' --json
cargo run -q -p langshell-cli --bin langshell -- validate -e 'open("/etc/passwd")' --json
cargo run -q -p langshell-cli --bin langshell -- session list
cargo run -q -p langshell-cli --bin langshell -- daemon --listen unix:///tmp/langshell.sock
```

The repository also includes shell scripts for the acceptance flows in [examples/README.md](./examples/README.md).

### JSON-RPC Request Shape

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

## Key Stable Contracts

According to the current design document, several constraints need to be locked down early in the MVP:

- Result capture priority: first the global `result` variable, then the last expression value, and only then stdout.
- Error codes must remain stable and machine-readable, including values such as `UNKNOWN_TOOL`, `PERMISSION_DENIED`, `RESULT_NOT_SERIALIZABLE`, and `TIMEOUT_WALL`.
- Snapshots must be versioned and validated against the capability set to avoid silently restoring into an incompatible environment.
- The sandbox must be zero-permission by default, with no direct access to the host filesystem, environment variables, subprocesses, or arbitrary network access.

These constraints directly shape the implementation of the CLI, daemon, SDK, and test matrix.

## Getting Started

### Requirements

- Rust stable toolchain with Edition 2024 support.
- Git submodules.
- Any supported macOS, Linux, or Windows environment.

### Clone the Repository

```bash
git clone --recurse-submodules <repo-url>
cd langshell
```

If you have already cloned the repository:

```bash
git submodule update --init --recursive
```

### Build and Test

The baseline checks for the workspace are:

```bash
cargo check
cargo test
```

### Try the End-to-End Examples

Run the acceptance scripts from the repository root:

```bash
bash examples/cli_single.sh
bash examples/session_reuse.sh
bash examples/validate_denied.sh
bash examples/snapshot_restore.sh
cargo run -q -p langshell --example sdk_async_fanout
```

To start the daemon manually:

```bash
cargo run -q -p langshell-cli --bin langshell -- daemon --listen unix:///tmp/langshell.sock
```

The CLI persists session snapshots under `LANGSHELL_SESSION_DIR` when set, or under the platform temporary directory by default.

## Near-Term Focus

The next implementation steps are the remaining V1 items from the product contract: a durable snapshot store, richer tool description stubs, more transport-backed capability modules, and broader security and compatibility coverage.

## Documentation

- [AGENTS.md](AGENTS.md): product requirements, engineering contracts, error codes, snapshots, and the test matrix.
- [SKILL.md](SKILL.md): how agents should use LangShell, including restrictions and best practices.

If RFCs, API references, or examples are added later, they should be placed under `docs/` and `examples/` and linked from this README.

## Roadmap

### MVP

- Monty integration.
- Persistent session state.
- Structured results and diagnostics output.
- Validate mode.
- Capability registry.
- Built-in file and HTTP capabilities.
- A minimal usable path across the CLI, daemon IPC, and Rust SDK.

### V1+

- Durable snapshot store.
- More complete typed stubs and tool-description injection.
- SQLite and object_store plugins.
- TypeScript and Deno backend.
- Multi-tenant daemon and remote execution support.

## License

Copyright © LDC Labs

Licensed under the Apache License, Version 2.0.