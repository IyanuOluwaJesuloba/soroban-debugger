import * as assert from 'assert';
import { ChildProcess, spawn } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import { DebuggerProcess, validateLaunchConfig, formatProtocolMismatchMessage, DebuggerTimeoutError } from '../cli/debuggerProcess';
import { resolveSourceBreakpoints } from '../dap/sourceBreakpoints';
import { DapClient } from './dapClient';

async function main(): Promise<void> {
  const compatibilityMessage = formatProtocolMismatchMessage({
    extensionVersion: '0.1.0',
    backendName: 'soroban-debug',
    backendVersion: '0.0.0',
    backendProtocolMin: 0,
    backendProtocolMax: 0,
    extra: 'Protocol mismatch: client supports [1..=1], server supports [0..=0]'
  });
  assert.match(compatibilityMessage, /Extension version:/, 'Expected protocol mismatch message to mention extension version');
  assert.match(compatibilityMessage, /supports protocol/, 'Expected protocol mismatch message to mention backend protocol range');
  assert.match(compatibilityMessage, /Remediation:/, 'Expected protocol mismatch message to include remediation guidance');

  await assertPerRequestTimeoutBehavior();

  const extensionRoot = process.cwd();
  const repoRoot = path.resolve(extensionRoot, '..', '..');

  const emittedFiles = [
    path.join(extensionRoot, 'dist', 'extension.js'),
    path.join(extensionRoot, 'dist', 'debugAdapter.js'),
    path.join(extensionRoot, 'dist', 'cli', 'debuggerProcess.js')
  ];

  for (const file of emittedFiles) {
    assert.ok(fs.existsSync(file), `Missing compiled artifact: ${file}`);
  }

  const preflightBinaryPath = emittedFiles[0];
  const contractPath = path.join(repoRoot, 'tests', 'fixtures', 'wasm', 'echo.wasm');
  assert.ok(fs.existsSync(contractPath), `Missing fixture WASM: ${contractPath}`);
  const snapshotPath = path.join(repoRoot, 'extensions', 'vscode', 'package.json');

  const goodPreflight = await validateLaunchConfig({
    binaryPath: preflightBinaryPath,
    contractPath,
    snapshotPath,
    entrypoint: 'echo',
    args: ['7'],
    token: 'debug-token'
  });
  assert.equal(goodPreflight.ok, true, 'Expected valid launch configuration to pass preflight');

  const missingContract = await validateLaunchConfig({
    binaryPath: preflightBinaryPath,
    contractPath: path.join(repoRoot, 'missing-contract.wasm'),
    entrypoint: 'echo',
    args: []
  });
  assert.equal(missingContract.ok, false, 'Expected missing contract path to fail preflight');
  assert.equal(missingContract.issues[0].field, 'contractPath');
  assert.match(missingContract.issues[0].message, /contractPath/);

  const badArgs = await validateLaunchConfig({
    binaryPath: preflightBinaryPath,
    contractPath,
    entrypoint: 'echo',
    args: [{ nested: undefined }]
  });
  assert.equal(badArgs.ok, false, 'Expected non-serializable args to fail preflight');
  assert.equal(badArgs.issues[0].field, 'args');
  assert.match(badArgs.issues[0].message, /\$\.0\.nested/);

  const badPort = await validateLaunchConfig({
    binaryPath: preflightBinaryPath,
    contractPath,
    entrypoint: 'echo',
    args: [],
    port: 70000
  });
  assert.equal(badPort.ok, false, 'Expected out-of-range port to fail preflight');
  assert.equal(badPort.issues[0].field, 'port');

  const badToken = await validateLaunchConfig({
    binaryPath: preflightBinaryPath,
    contractPath,
    entrypoint: 'echo',
    args: [],
    token: '   '
  });
  assert.equal(badToken.ok, false, 'Expected blank token to fail preflight');
  assert.equal(badToken.issues[0].field, 'token');

  const binaryPath = process.env.SOROBAN_DEBUG_BIN
    || path.join(repoRoot, 'target', 'debug', process.platform === 'win32' ? 'soroban-debug.exe' : 'soroban-debug');

  if (!fs.existsSync(binaryPath)) {
    console.log(`Skipping debugger smoke test because the CLI binary was not found at ${binaryPath}`);
    return;
  }

  const debuggerProcess = new DebuggerProcess({
    binaryPath,
    contractPath,
    entrypoint: 'echo',
    args: ['7']
  });

  await debuggerProcess.start();
  await debuggerProcess.ping();

  const sourcePath = path.join(repoRoot, 'tests', 'fixtures', 'contracts', 'echo', 'src', 'lib.rs');
  assert.ok(fs.existsSync(sourcePath), `Missing fixture source: ${sourcePath}`);
  const exportedFunctions = await debuggerProcess.getContractFunctions();
  const resolvedBreakpoints = resolveSourceBreakpoints(sourcePath, [10], exportedFunctions);
  assert.equal(resolvedBreakpoints[0].verified, true, 'Expected echo breakpoint to resolve');
  assert.equal(resolvedBreakpoints[0].functionName, 'echo');

  await debuggerProcess.setBreakpoint({
    id: 'echo',
    functionName: 'echo'
  });
  const paused = await debuggerProcess.execute();
  assert.equal(paused.paused, true, 'Expected breakpoint to pause before execution');

  const pausedInspection = await debuggerProcess.inspect();
  assert.match(pausedInspection.args || '', /7/, 'Expected paused inspection to include call args');

  const resumed = await debuggerProcess.continueExecution();
  assert.match(resumed.output || '', /7/, 'Expected continue() to finish echo()');
  await debuggerProcess.clearBreakpoint('echo');

  const result = await debuggerProcess.execute();
  assert.match(result.output, /7/, 'Expected second echo() to return the input');

  const inspection = await debuggerProcess.inspect();
  assert.ok(Array.isArray(inspection.callStack), 'Expected call stack array from inspection');
  assert.match(inspection.args || '', /7/, 'Expected inspection to include args');

  const storage = await debuggerProcess.getStorage();
  assert.ok(typeof storage === 'object' && storage !== null, 'Expected storage snapshot object');

  await debuggerProcess.stop();
  console.log('VS Code extension smoke tests passed');

  // DAP end-to-end tests (adapter <-> backend).
  const debugAdapterPath = path.join(extensionRoot, 'dist', 'debugAdapter.js');
  assert.ok(fs.existsSync(debugAdapterPath), `Missing debug adapter entrypoint: ${debugAdapterPath}`);

  await runDapHappyPathE2E(debugAdapterPath, {
    contractPath,
    sourcePath,
    binaryPath
  });
  await runDapLaunchErrorE2E(debugAdapterPath, {
    contractPath: path.join(repoRoot, 'tests', 'fixtures', 'wasm', 'does-not-exist.wasm'),
    sourcePath,
    binaryPath
  });

  console.log('VS Code DAP end-to-end tests passed');
}

async function assertPerRequestTimeoutBehavior(): Promise<void> {
  const dp = new DebuggerProcess({
    contractPath: 'placeholder.wasm',
    entrypoint: 'main',
    args: [],
    requestTimeoutMs: 5
  });

  (dp as any).socket = { write: () => undefined, destroyed: false };

  const sendRequest = (dp as any).sendRequest.bind(dp) as (req: any, opts?: any) => Promise<any>;

  for (const req of [
    { type: 'Handshake', client_name: 'test', client_version: '0.0.0', protocol_min: 1, protocol_max: 1 },
    { type: 'Inspect' },
    { type: 'GetStorage' },
    { type: 'Continue' }
  ]) {
    let threwTimeout = false;
    try {
      await sendRequest(req, { timeoutMs: 5 });
    } catch (error) {
      threwTimeout = error instanceof DebuggerTimeoutError;
    }

    assert.equal(threwTimeout, true, `Expected ${req.type} to time out deterministically`);
    assert.equal((dp as any).pendingRequests.size, 0, 'Expected pending request map to be cleared after timeout');
  }
}

async function runDapHappyPathE2E(
  debugAdapterPath: string,
  fixtures: { contractPath: string; sourcePath: string; binaryPath: string }
): Promise<void> {
  const proc = spawn(process.execPath, [debugAdapterPath], {
    stdio: ['pipe', 'pipe', 'pipe']
  });
  const client = new DapClient(proc);

  try {
    const init = await client.request('initialize', {
      adapterID: 'soroban',
      linesStartAt1: true,
      columnsStartAt1: true,
      pathFormat: 'path'
    });
    assert.equal(init.success, true, `initialize failed: ${init.message || ''}`);
    await client.waitForEvent('initialized');

    const launch = await client.request('launch', {
      type: 'soroban',
      request: 'launch',
      name: 'Soroban: E2E',
      contractPath: fixtures.contractPath,
      entrypoint: 'echo',
      args: ['7'],
      trace: false,
      binaryPath: fixtures.binaryPath
    }, 30_000);
    assert.equal(launch.success, true, `launch failed: ${launch.message || ''}`);

    const setBps = await client.request('setBreakpoints', {
      source: { path: fixtures.sourcePath },
      breakpoints: [{ line: 10 }]
    });
    assert.equal(setBps.success, true, `setBreakpoints failed: ${setBps.message || ''}`);
    assert.equal(setBps.body?.breakpoints?.[0]?.verified, true, 'Expected breakpoint to verify');

    const configDone = await client.request('configurationDone', {});
    assert.equal(configDone.success, true, `configurationDone failed: ${configDone.message || ''}`);

    await client.waitForEvent('stopped', (e) => e.body?.reason === 'entry');

    const cont = await client.request('continue', { threadId: 1 }, 30_000);
    assert.equal(cont.success, true, `continue failed: ${cont.message || ''}`);

    await client.waitForEvent('stopped', (e) => e.body?.reason === 'breakpoint', 30_000);

    const threads = await client.request('threads', {});
    assert.equal(threads.success, true);
    assert.equal(Array.isArray(threads.body?.threads), true, 'Expected threads array');

    const stack = await client.request('stackTrace', { threadId: 1 });
    assert.equal(stack.success, true);
    const frameId = stack.body?.stackFrames?.[0]?.id;
    assert.ok(frameId, 'Expected at least one stack frame');

    const scopes = await client.request('scopes', { frameId });
    assert.equal(scopes.success, true);
    const argsScope = (scopes.body?.scopes || []).find((s: any) => s.name === 'Arguments');
    assert.ok(argsScope?.variablesReference, 'Expected Arguments scope');

    const argsVars = await client.request('variables', { variablesReference: argsScope.variablesReference });
    assert.equal(argsVars.success, true);
    assert.match(JSON.stringify(argsVars.body?.variables || []), /7/, 'Expected argument variable to include the input');

    const evalArgs = await client.request('evaluate', { expression: 'args', frameId });
    assert.equal(evalArgs.success, true);
    assert.match(String(evalArgs.body?.result || ''), /7/, 'Expected evaluate(args) to include the input');

    const evalStorage = await client.request('evaluate', { expression: 'storage', frameId });
    assert.equal(evalStorage.success, true);
    assert.match(String(evalStorage.body?.result || ''), /^\{/, 'Expected evaluate(storage) to return JSON');

    // Exercise stepping commands (these may exit quickly depending on the contract).
    const stepIn = await client.request('stepIn', { threadId: 1 }, 30_000);
    assert.equal(stepIn.success, true);
    const afterStepIn = await client.waitForAnyEvent(['stopped', 'exited'], () => true, 30_000);

    if (afterStepIn.event === 'stopped') {
      const next = await client.request('next', { threadId: 1 }, 30_000);
      assert.equal(next.success, true);
      const afterNext = await client.waitForAnyEvent(['stopped', 'exited'], () => true, 30_000);

      if (afterNext.event === 'stopped') {
        const stepOut = await client.request('stepOut', { threadId: 1 }, 30_000);
        assert.equal(stepOut.success, true);
        await client.waitForAnyEvent(['stopped', 'exited'], () => true, 30_000);
      }
    }

    // Finish execution.
    const cont2 = await client.request('continue', { threadId: 1 }, 30_000);
    assert.equal(cont2.success, true);
    await client.waitForEvent('exited', () => true, 30_000);

    const disconnect = await client.request('disconnect', { restart: false });
    assert.equal(disconnect.success, true);
  } finally {
    client.dispose();
  }
}

async function runDapLaunchErrorE2E(
  debugAdapterPath: string,
  fixtures: { contractPath: string; sourcePath: string; binaryPath: string }
): Promise<void> {
  const proc = spawn(process.execPath, [debugAdapterPath], {
    stdio: ['pipe', 'pipe', 'pipe']
  });
  const client = new DapClient(proc);

  try {
    const init = await client.request('initialize', {
      adapterID: 'soroban',
      linesStartAt1: true,
      columnsStartAt1: true,
      pathFormat: 'path'
    });
    assert.equal(init.success, true);
    await client.waitForEvent('initialized');

    const launch = await client.request('launch', {
      type: 'soroban',
      request: 'launch',
      name: 'Soroban: E2E (error)',
      contractPath: fixtures.contractPath,
      entrypoint: 'echo',
      args: ['7'],
      trace: false,
      binaryPath: fixtures.binaryPath
    }, 30_000);
    assert.equal(launch.success, false, 'Expected launch to fail for missing contract fixture');

    const disconnect = await client.request('disconnect', { restart: false });
    assert.equal(disconnect.success, true);
  } finally {
    client.dispose();
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
