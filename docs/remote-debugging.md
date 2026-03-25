# Remote Debugging Guide

## Overview

The Soroban Debugger supports remote debugging, allowing you to debug smart contracts running in CI environments, remote servers, or isolated systems from your local development machine. This enables powerful debugging workflows for production-like scenarios.

> **Note: Remote client mode is CLI-only.** The `soroban-debug remote` command and TLS configuration are not available through the VS Code extension. The extension spawns and manages the debug server locally as a subprocess. If you need to debug against a remote server from VS Code, see the [VS Code Extension and Remote Mode](#vs-code-extension-and-remote-mode) section below. For a full breakdown of what each surface supports, see the [Feature Matrix](feature-matrix.md#remote-debugging).

## Architecture

The remote debugging feature consists of three main components:

1. **Debug Server** - Runs on the remote system, hosts the contract execution environment
2. **Remote Client** - Connects from your local machine to issue debug commands
3. **Wire Protocol** - JSON-over-TCP communication protocol for debug operations

## Quick Start

### Starting the Debug Server

On the remote system (or CI environment):

```bash
# Basic server (no authentication)
soroban-debug server --port 9229

# With token authentication
soroban-debug server --port 9229 --token mySecretToken123

# With TLS encryption
soroban-debug server --port 9229 \
  --token mySecretToken123 \
  --tls-cert /path/to/cert.pem \
  --tls-key /path/to/key.pem
```

### Connecting from Local Machine

```bash
# Connect and execute a function
soroban-debug remote \
  --remote localhost:9229 \
  --token mySecretToken123 \
  --contract ./contract.wasm \
  --function increment \
  --args '["user1", 100]'

# Just ping the server
soroban-debug remote \
  --remote localhost:9229 \
  --token mySecretToken123
```

## Features

> **VS Code Extension users:** When you launch a debug session in VS Code, the extension automatically starts `soroban-debug server` as a local subprocess and connects to it. The `port` and `token` fields in `launch.json` configure this local server. You cannot use the extension to connect to a pre-existing remote server — that requires the CLI `remote` command. TLS is not configurable from the extension. If you need TLS or remote-client connectivity, use the CLI directly or see the [VS Code Extension and Remote Mode](#vs-code-extension-and-remote-mode) section.

### Supported Debug Operations

The debugger supports all core debugging operations over TCP:

- **Contract Loading** - Load WASM contracts onto the server
- **Function Execution** - Execute contract functions with arguments
- **Breakpoints** - Set, clear, and list function breakpoints
- **Step Debugging** - Step through execution
- **State Inspection** - Inspect current execution state, call stack
- **Storage Access** - Get and set contract storage
- **Budget Information** - Monitor CPU and memory consumption
- **Snapshot Loading** - Load network snapshots

### Authentication

Token-based authentication prevents unauthorized access:

```bash
# Server with token
soroban-debug server --port 9229 --token "your-secret-token-here"

# Client provides matching token
soroban-debug remote --remote host:9229 --token "your-secret-token-here"
```

### TLS Encryption

Secure your debug sessions with TLS:

```bash
# Generate self-signed certificate (for testing)
openssl req -x509 -newkey rsa:4096 \
  -keyout key.pem -out cert.pem \
  -days 365 -nodes \
  -subj "/CN=localhost"

# Start server with TLS
soroban-debug server --port 9229 \
  --tls-cert cert.pem \
  --tls-key key.pem \
  --token myToken
```

### VS Code Extension and Remote Mode

The VS Code extension uses the debug server protocol internally, but does not expose
the full remote client capability to users:

| Capability | CLI | VS Code Extension |
|---|---|---|
| Start server (listen on port) | `soroban-debug server --port N` | Automatic — extension-managed |
| Configure server port | `--port N` | `"port"` in `launch.json` |
| Configure auth token | `--token T` on `server` | `"token"` in `launch.json` |
| Connect to remote server | `soroban-debug remote --remote host:N` | **Not supported** |
| TLS encryption | `--tls-cert` / `--tls-key` | **Not supported** |

**Workaround for VS Code users who need a remote server:** use an SSH tunnel to
forward the remote port to your local machine, then configure the extension to
connect to the local end of the tunnel:

```bash
# On the remote machine
soroban-debug server --port 9229 --token $MY_TOKEN

# On your local machine (in a separate terminal)
ssh -L 9229:localhost:9229 user@remote-host
```

Then in your `.vscode/launch.json`:
```json
{
  "type": "soroban",
  "request": "launch",
  "name": "Debug via SSH tunnel",
  "contractPath": "${workspaceFolder}/contract.wasm",
  "port": 9229,
  "token": "${env:MY_TOKEN}"
}
```

The extension will connect through the tunnel as if the server were local. Note
that the extension still manages the server subprocess normally — the tunnel
approach is for pointing the extension at a manually started remote server by
binding the same port locally.

## Wire Protocol

The debug protocol uses JSON messages over TCP with line-delimited encoding.

### Protocol Compatibility Matrix

The backend and adapter negotiate a protocol version during the required `Handshake` request.
If there is no overlap in supported versions, the session fails fast with an actionable error.

| Wire protocol | Backend (`soroban-debug`) | VS Code extension | Notes |
| --- | --- | --- | --- |
| 1 | >= 0.1.0 | >= 0.1.0 | Handshake required; highest common version selected |

### Message Format

```json
{
  "id": 1,
  "request": { ... }
}
```

```json
{
  "id": 1,
  "response": { ... }
}
```

### Request Types

#### Handshake (Required)

Clients MUST negotiate a compatible protocol version before issuing other debug requests.

```json
{
  "type": "Handshake",
  "client_name": "vscode-extension",
  "client_version": "0.1.0",
  "protocol_min": 1,
  "protocol_max": 1
}
```

#### Authenticate
```json
{
  "type": "Authenticate",
  "token": "your-token-here"
}
```

#### LoadContract
```json
{
  "type": "LoadContract",
  "contract_path": "/path/to/contract.wasm"
}
```

#### Execute
```json
{
  "type": "Execute",
  "function": "increment",
  "args": "[\"user1\", 100]"
}
```

#### Step
```json
{
  "type": "Step"
}
```

#### SetBreakpoint
```json
{
  "type": "SetBreakpoint",
  "function": "transfer"
}
```

#### Inspect
```json
{
  "type": "Inspect"
}
```

#### GetStorage
```json
{
  "type": "GetStorage"
}
```

#### GetStack
```json
{
  "type": "GetStack"
}
```

#### GetBudget
```json
{
  "type": "GetBudget"
}
```

### Response Types

#### HandshakeAck
```json
{
  "type": "HandshakeAck",
  "server_name": "soroban-debug",
  "server_version": "0.1.0",
  "protocol_min": 1,
  "protocol_max": 1,
  "selected_version": 1
}
```

#### IncompatibleProtocol
```json
{
  "type": "IncompatibleProtocol",
  "message": "Protocol mismatch: ... Upgrade the older component.",
  "server_name": "soroban-debug",
  "server_version": "0.1.0",
  "protocol_min": 1,
  "protocol_max": 1
}
```

#### Authenticated
```json
{
  "type": "Authenticated",
  "success": true,
  "message": "Authentication successful"
}
```

#### ContractLoaded
```json
{
  "type": "ContractLoaded",
  "size": 123456
}
```

#### ExecutionResult
```json
{
  "type": "ExecutionResult",
  "success": true,
  "output": "Ok(100)",
  "error": null
}
```

#### StepResult
```json
{
  "type": "StepResult",
  "paused": true,
  "current_function": "transfer",
  "step_count": 42
}
```

#### InspectionResult
```json
{
  "type": "InspectionResult",
  "function": "transfer",
  "step_count": 42,
  "paused": true,
  "call_stack": ["main", "transfer", "validate"]
}
```

## Use Cases

### CI/CD Debugging

Debug contracts in your CI pipeline:

```yaml
# .github/workflows/debug.yml
steps:
  - name: Start Debug Server
    run: |
      soroban-debug server --port 9229 --token ${{ secrets.DEBUG_TOKEN }} &
      sleep 2

  - name: Debug Contract
    run: |
      soroban-debug remote \
        --remote localhost:9229 \
        --token ${{ secrets.DEBUG_TOKEN }} \
        --contract ./target/wasm32-unknown-unknown/release/contract.wasm \
        --function test_function \
        --args '[1, 2, 3]'
```

### Remote Server Debugging

Debug contracts on staging/production environments:

```bash
# On remote server
ssh user@staging-server
soroban-debug server --port 9229 --token $TOKEN --tls-cert cert.pem --tls-key key.pem

# From local machine (with SSH tunnel)
ssh -L 9229:localhost:9229 user@staging-server
soroban-debug remote --remote localhost:9229 --token $TOKEN --contract local.wasm
```

### Team Debugging Sessions

Multiple developers can connect to the same debug server:

```bash
# Team member starts server
soroban-debug server --port 9229 --token team-debug-session

# Other team members connect
soroban-debug remote --remote team-lead-ip:9229 --token team-debug-session
```

## Security Best Practices

1. **Always use authentication** in production environments
2. **Enable TLS** for remote connections over the internet
3. **Use strong tokens** - Generate cryptographically random tokens
4. **Firewall rules** - Restrict server port access to known IPs
5. **Rotate tokens** regularly for long-running servers
6. **Monitor connections** - Log all connection attempts

### Generating Secure Tokens

```bash
# Generate a secure random token
openssl rand -hex 32
```

## Troubleshooting

### Connection Refused

```bash
# Check server is running
netstat -an | grep 9229

# Check firewall allows connections
sudo ufw allow 9229/tcp

# Test basic connectivity
telnet host 9229
```

### Authentication Failed

- Verify token matches on both server and client
- Check for whitespace in token strings
- Ensure token was properly set when starting server

### TLS Handshake Errors
- Verify certificate and key paths are correct
- Check certificate hasn't expired
- Ensure client trusts the certificate (or use self-signed for testing)

## Advanced Usage

### Custom Protocol Extensions

The debug protocol can be extended with custom request/response types:

```rust
// Add to src/server/protocol.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DebugRequest {
    // ... existing variants ...
    CustomCommand { data: String },
}
```

### Programmatic Client Usage

Use the RemoteClient API directly in Rust:

```rust
use soroban_debugger::client::RemoteClient;

let mut client = RemoteClient::connect("localhost:9229", Some("token".to_string()))?;

client.load_contract("contract.wasm")?;
let result = client.execute("increment", Some("[100]"))?;
println!("Result: {}", result);

client.set_breakpoint("transfer")?;
client.step()?;

let (function, step_count, paused, stack) = client.inspect()?;
println!("At function: {:?}, steps: {}", function, step_count);
```

## Future Enhancements

Planned features for remote debugging:

- [ ] WebSocket support for browser-based debugging
- [ ] Multi-session support (concurrent debug sessions)
- [ ] Session recording and replay
- [ ] Visual debugger UI (web interface)
- [ ] Performance profiling over network
- [ ] Distributed debugging (multiple contracts across nodes)

## Related Documentation

- [Plugin API](plugin-api.md) - Extend debugger with custom plugins
- [Storage Snapshots](storage-snapshot.md) - Load network state for debugging
- [Instruction Stepping](instruction-stepping.md) - Low-level instruction debugging

## Contributing

To contribute to remote debugging features:

1. Review the [CONTRIBUTING.md](../CONTRIBUTING.md) guide
2. Check existing issues tagged `remote-debugging`
3. Propose enhancements in GitHub Discussions
4. Submit PRs with tests and documentation

## License

This feature is part of the Soroban Debugger project, licensed under MIT OR Apache-2.0.
