# oxi-sandbox

Platform-specific sandboxed command execution for the [oxi](https://github.com/a7garden/oxi) agent.

Wraps a child process in the host's native sandbox mechanism based on a
[`SandboxProfile`](src/lib.rs) policy:

| Platform | Mechanism                | Feature             |
|----------|--------------------------|---------------------|
| macOS    | `sandbox-exec(1)`        | —                   |
| Linux    | `bwrap(1)` (bubblewrap)  | `linux-bwrap`       |
| Other    | direct execution (noop)  | —                   |

Each runner falls back to unsandboxed execution when the host tool is missing
(e.g. inside containers), so callers always get a best-effort isolation layer.

This crate is part of the oxi workspace but is independent — no other `oxi-*`
crate depends on it.

## License

MIT
