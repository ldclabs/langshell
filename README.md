# LangShell

> Stateful, capability-scoped, sandboxed code execution for AI agents.

**[English](./README.md) | [中文](./README_CN.md)**

LangShell is a secure execution layer for AI agents. Its goal is to let an agent produce a piece of Python code that can be validated, resumed, and audited to complete complex work, instead of decomposing everything into a large number of fragile tool calls.

The project is implemented in Rust. The MVP uses Pydantic Monty as the Python-subset execution engine. The core idea behind LangShell is simple: treat code as the interface, sessions as the unit of state, and host-registered capabilities as the only entry points to the outside world.

## Current Status

This repository is still at the bootstrap skeleton stage.

- The Cargo workspace, crate split, Monty dependency patch, and design contract documents are already in place.
- The runtime, CLI binary, built-in tools, examples, and end-to-end tests are not implemented yet.
- [AGENTS.md](AGENTS.md) is currently the most complete source of product requirements and engineering contracts.
- [SKILL.md](SKILL.md) describes how an AI agent is expected to use LangShell safely in the future.

This README is written as a product overview, repository guide, and implementation entry point. It explicitly distinguishes planned capabilities from what is actually implemented today.

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

## Target Capabilities

The LangShell MVP is intended to provide:

- Stateful execution of a Python subset.
- Top-level await and async capability calls.
- Validate and dry-run modes that catch syntax, type, permission, and tool-availability issues without causing side effects.
- A capability registry so the host can expose controlled external functions.
- Capability discovery interfaces such as list_tools, describe_tool, and current_policy.
- Structured results, stdout and stderr capture, diagnostics, and stable error codes.
- Limits for timeout, cancellation, output size, memory, and external call counts.
- Snapshot and restore for resumability and approval-boundary pauses.

The MVP prioritizes controlled file and HTTP access, including capabilities such as read_text, write_text, list_dir, fetch_text, and fetch_json.

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

Each crate is still an initialization skeleton, but the directory boundaries already align with the engineering contract defined in the product documentation and are ready for implementation work.

## Target Interface Examples

The following examples describe the intended LangShell experience. They do not imply that these commands or behaviors are already implemented in this repository.

### Agent-Side Python

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

### Target CLI Shape

```bash
langshell run -e 'result = sum(range(10))' --json
langshell validate -f script.py --session-id agent-123
langshell daemon --listen unix:///tmp/langshell.sock
langshell session snapshot agent-123 --out snapshot.bin
```

### Target JSON-RPC Shape

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

### Basic Checks Available Today

While the repository is still in the skeleton stage, the most useful checks are:

```bash
cargo check
cargo test
```

As the runtime and CLI are implemented, examples, end-to-end coverage, and the security test matrix should be added incrementally.

## Suggested Implementation Order

If you plan to build the MVP from this skeleton, a reasonable order is:

1. Define stable data structures, error codes, and trait boundaries in `langshell-core`.
2. Complete Monty execution integration and result capture in `langshell-monty`.
3. Implement the minimum built-in capabilities in `langshell-tools`: file read/write, directory listing, and HTTP fetch.
4. Provide a builder and session execution interface in `langshell`.
5. Implement the run, validate, daemon, session, and tools commands in `langshell-cli`.
6. Add snapshots, JSON-RPC support, end-to-end examples, and the security test matrix.

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