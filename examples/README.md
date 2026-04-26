# LangShell MVP E2E Examples

Run these from the repository root.

```bash
bash examples/cli_single.sh
bash examples/session_reuse.sh
bash examples/validate_denied.sh
bash examples/snapshot_restore.sh
cargo run -q -p langshell --example sdk_async_fanout
```

The daemon example is line-delimited JSON-RPC over a Unix socket. Start the daemon:

```bash
cargo run -q -p langshell-cli --bin langshell -- daemon --listen unix:///tmp/langshell.sock
```

Then send requests such as `session.create`, `session.run`, `session.snapshot`, and `session.destroy` with any Unix-socket client.