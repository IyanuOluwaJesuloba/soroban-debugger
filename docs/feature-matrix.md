# Soroban Debugger Feature Matrix

This document is the authoritative reference for which debugging features are
available in each surface: the **CLI** (`soroban-debug` binary) and the
**VS Code Extension** (Debug Adapter Protocol session).

Legend:
- **YES** — fully supported
- **NO** — not supported; the option does not exist in that surface or produces an explicit error
- **PARTIAL** — supported with noted limitations

> For questions about feature gaps, see the [FAQ](faq.md#cli-vs-vs-code-extension---feature-differences).

---

## Stepping

| Feature | CLI flag / command | VS Code Extension | Notes |
|---|---|---|---|
| Step over (next) | `n` in interactive/REPL, `--step-mode over` | YES — F10 / Step Over | Shared execution engine. |
| Step into | `s` in interactive/REPL, `--step-mode into` | YES — F11 / Step In | Default step mode on both surfaces. |
| Step out | `o` in interactive/REPL, `--step-mode out` | YES — Shift+F11 / Step Out | |
| Continue to next breakpoint | `c` in interactive mode | YES — F5 / Continue | |
| Instruction-level stepping | `--instruction-debug`, `--step-instructions` | NO | WASM opcode-level stepping is CLI-only. No DAP equivalent exists. |
| Step mode selection | `--step-mode [into\|over\|out\|block]` | NO | Step granularity is fixed at function boundary in the extension. |
| Block-level stepping | `--step-mode block` | NO | CLI-only. |

---

## Breakpoints

| Feature | CLI flag / command | VS Code Extension | Notes |
|---|---|---|---|
| Function breakpoints | `-b`/`--breakpoint <name>` (repeatable) | YES — click line in gutter | Both surfaces target function names. The extension resolves clicked source lines to the enclosing exported function via `resolveSourceBreakpoints`. |
| Source / line breakpoints | NO | PARTIAL | The extension maps source line clicks to function boundaries. Execution pauses at the function entry point, not the exact clicked line. |
| Conditional breakpoints | NO | NO | `supportsConditionalBreakpoints = false` in `initializeRequest`. |
| Hit-count conditions | NO | NO | `supportsHitConditionalBreakpoints = false` in `initializeRequest`. |
| Log points | NO | NO | `supportsLogPoints = false` in `initializeRequest`. |
| Set variable at breakpoint | NO | NO | `supportsSetVariable = false` in `initializeRequest`. Read-only inspection only. |

---

## Evaluate / Inspect

| Feature | CLI flag / command | VS Code Extension | Notes |
|---|---|---|---|
| Expression evaluation (paused) | `eval` in interactive/REPL session | YES — Debug Console when paused | Extension requires `isPaused = true`. |
| Hover evaluation | N/A | YES | `supportsEvaluateForHovers = true`. |
| Variable inspection — storage | `--export-storage`, interactive `storage` command | YES — Variables panel → Storage scope | Extension shows storage snapshot at current pause point. |
| Variable inspection — arguments | interactive session | YES — Variables panel → Arguments scope | |
| Call stack inspection | interactive `stack` command | YES — up to 50 frames | Adapter slices `callStack.slice(0, 50)`. |

---

## Auth Display

| Feature | CLI flag / command | VS Code Extension | Notes |
|---|---|---|---|
| Show authorization tree | `--show-auth` | NO | No `showAuth` field in launch configuration. No DAP equivalent. |
| Auth filtering | not yet implemented | NO | |

---

## Storage Filters

| Feature | CLI flag / command | VS Code Extension | Notes |
|---|---|---|---|
| Prefix filter (`balance:*`) | `--storage-filter 'balance:*'` (repeatable) | NO | No `storageFilter` launch config field. All storage keys are shown unfiltered in the Variables panel. |
| Regex filter (`re:<pattern>`) | `--storage-filter 're:^user_\d+$'` | NO | |
| Exact-key filter | `--storage-filter exact_key` | NO | |
| Export storage after execution | `--export-storage <file>` | NO | |
| Import storage before execution | `--import-storage <file>` | PARTIAL | Use `snapshotPath` in `launch.json` for initial contract state instead. |

---

## Remote Debugging

| Feature | CLI flag / command | VS Code Extension | Notes |
|---|---|---|---|
| Start debug server | `soroban-debug server --port <n>` | PARTIAL — automatic | The extension automatically spawns `soroban-debug server` as a local subprocess via `DebuggerProcess`. |
| Configure server port | `--port <n>` on `server` command | YES — `"port"` in `launch.json` | |
| Configure auth token | `--token <t>` on `server` command | YES — `"token"` in `launch.json` | |
| Connect as remote client | `soroban-debug remote --remote <host:port>` | NO | The extension only manages a local server subprocess. Connecting to a pre-existing remote server is not supported from the extension. |
| TLS encryption — server | `--tls-cert <file> --tls-key <file>` on `server` | NO | No TLS config fields in `launch.json`. |
| TLS encryption — client | `--tls-cert`/`--tls-key` on `remote` | NO | |

---

## Batch Execution

| Feature | CLI flag / command | VS Code Extension | Notes |
|---|---|---|---|
| Batch arguments from file | `--batch-args <file.json>` | NO | No `batchArgs` field in `launch.json`. |
| Repeat execution N times | `--repeat <n>` | NO | |

---

## CLI-Exclusive Subcommands

These subcommands have no VS Code Extension equivalent. They are accessible only
from the CLI and are not reachable via a DAP session.

| Subcommand | Description |
|---|---|
| `soroban-debug analyze` | Static and dynamic security vulnerability analysis |
| `soroban-debug symbolic` | Symbolic execution over the contract's input space |
| `soroban-debug optimize` | Gas optimization suggestions |
| `soroban-debug profile` | Execution hotspot profiling |
| `soroban-debug compare` | Side-by-side trace comparison between two executions |
| `soroban-debug replay` | Replay execution from a previously exported trace file |
| `soroban-debug upgrade-check` | Compatibility check between two contract WASM versions |
| `soroban-debug scenario` | Multi-step scenario execution from a TOML file |
| `soroban-debug tui` | Full-screen TUI dashboard |
| `soroban-debug repl` | Interactive REPL for contract exploration |

---

## Launch Configuration Field Mapping

For VS Code users, this table maps CLI flags to their `launch.json` equivalents.

| CLI flag | `launch.json` field | Available in extension |
|---|---|---|
| `--contract` | `contractPath` | YES |
| `--network-snapshot` / `--snapshot` | `snapshotPath` | YES |
| `--function` | `entrypoint` | YES |
| `--args` | `args` | YES |
| `--port` | `port` | YES |
| `--token` | `token` | YES |
| `--breakpoint` | Set via editor gutter clicks | YES |
| `--storage-filter` | (none) | NO |
| `--show-auth` | (none) | NO |
| `--instruction-debug` | (none) | NO |
| `--step-instructions` | (none) | NO |
| `--step-mode` | (none) | NO |
| `--batch-args` | (none) | NO |
| `--repeat` | (none) | NO |
| `--tls-cert` / `--tls-key` | (none) | NO |
| `--import-storage` | Use `snapshotPath` instead | PARTIAL |
| `--export-storage` | (none) | NO |
| `--show-events` | (none) | NO |
| `--event-filter` | (none) | NO |
| `--dry-run` | (none) | NO |
| `--mock` | (none) | NO |

---

## Maintaining This Document

This matrix is derived from:
- **CLI surface:** `src/cli/args.rs` — `RunArgs`, `InteractiveArgs`, `ServerArgs`, `RemoteArgs` structs
- **DAP surface:** `extensions/vscode/src/dap/adapter.ts` — `initializeRequest` capability flags and `launchRequest` argument handling

When adding a new CLI flag or DAP capability, update this file alongside the
implementation to keep gaps explicit rather than implicit.
