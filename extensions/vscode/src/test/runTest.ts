import * as assert from 'assert';
import * as fs from 'fs';
import * as net from 'net';
import * as path from 'path';
import { DebuggerProcess } from '../cli/debuggerProcess';
import { resolveSourceBreakpoints } from '../dap/sourceBreakpoints';
import { VariableStore } from '../dap/variableStore';

type DebugMessage = {
  id: number;
  request?: { type: string; [key: string]: unknown };
  response?: { type: string; [key: string]: unknown };
};

async function startMockDebuggerServer(options: { evaluateDelayMs: number }): Promise<{ port: number; close: () => Promise<void> }> {
  const server = net.createServer();
  const sockets = new Set<net.Socket>();

  server.on('connection', (socket) => {
    sockets.add(socket);
    socket.setEncoding('utf8');

    let buffer = '';
    socket.on('data', (chunk: string) => {
      buffer += chunk;
      while (true) {
        const newlineIndex = buffer.indexOf('\n');
        if (newlineIndex === -1) {
          return;
        }

        const line = buffer.slice(0, newlineIndex).trim();
        buffer = buffer.slice(newlineIndex + 1);
        if (!line) {
          continue;
        }

        const message = JSON.parse(line) as DebugMessage;
        if (!message.request) {
          continue;
        }

        const respond = (response: DebugMessage['response'], delayMs = 0) => {
          setTimeout(() => {
            if (socket.destroyed) {
              return;
            }
            socket.write(`${JSON.stringify({ id: message.id, response })}\n`);
          }, delayMs);
        };

        switch (message.request.type) {
          case 'Authenticate':
            respond({ type: 'Authenticated', success: true, message: 'ok' });
            break;
          case 'LoadSnapshot':
            respond({ type: 'SnapshotLoaded', summary: 'ok' });
            break;
          case 'LoadContract':
            respond({ type: 'ContractLoaded', size: 0 });
            break;
          case 'Ping':
            respond({ type: 'Pong' });
            break;
          case 'Evaluate':
            respond({ type: 'EvaluateResult', result: 'ok', result_type: 'string', variables_reference: 0 }, options.evaluateDelayMs);
            break;
          case 'Inspect':
            respond({ type: 'InspectionResult', function: 'main', args: '[]', step_count: 0, paused: true, call_stack: ['main'] }, options.evaluateDelayMs);
            break;
          case 'GetStorage':
            respond({ type: 'StorageState', storage_json: '{}' }, options.evaluateDelayMs);
            break;
          case 'Disconnect':
            respond({ type: 'Disconnected' });
            break;
          default:
            respond({ type: 'Error', message: `Unhandled request type: ${message.request.type}` });
            break;
        }
      }
    });

    socket.on('close', () => sockets.delete(socket));
    socket.on('error', () => sockets.delete(socket));
  });

  const port = await new Promise<number>((resolve, reject) => {
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (!address || typeof address === 'string') {
        reject(new Error('Failed to allocate mock server port'));
        return;
      }
      resolve(address.port);
    });
    server.on('error', reject);
  });

  return {
    port,
    close: async () => {
      for (const socket of sockets) {
        socket.destroy();
      }
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  };
}

async function wait(ms: number): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, ms));
}

async function main(): Promise<void> {
  const extensionRoot = process.cwd();
  const repoRoot = path.resolve(extensionRoot, '..', '..');

  {
    const store = new VariableStore({ pageSize: 3, maxStringPreview: 6, maxHexPreviewBytes: 2 });

    const bigArray = [1, 2, 3, 4, 5, 6, 7];
    const arrayVar = store.toVariable('arr', bigArray);
    assert.ok(arrayVar.variablesReference && arrayVar.variablesReference > 0, 'Expected array to be expandable');
    assert.equal(arrayVar.indexedVariables, bigArray.length);

    const firstPage = store.getVariables(arrayVar.variablesReference as number);
    assert.deepEqual(firstPage.slice(0, 3).map((v) => v.name), ['[0]', '[1]', '[2]']);
    assert.equal(firstPage[0].value, '1');

    const pager = firstPage[3];
    assert.match(pager.name, /show more/i);
    assert.ok(pager.variablesReference && pager.variablesReference > 0, 'Expected pager to be expandable');

    const secondPage = store.getVariables(pager.variablesReference as number);
    assert.deepEqual(secondPage.slice(0, 3).map((v) => v.name), ['[3]', '[4]', '[5]']);
    assert.equal(secondPage[2].value, '6');

    const thirdPager = secondPage[3];
    const thirdPage = store.getVariables(thirdPager.variablesReference as number);
    assert.deepEqual(thirdPage.map((v) => v.name), ['[6]']);

    const longString = store.toVariable('s', '1234567890');
    assert.ok(longString.variablesReference && longString.variablesReference > 0, 'Expected long string to be expandable');
    assert.match(longString.value, /truncated/i);
    const fullString = store.getVariables(longString.variablesReference as number);
    assert.equal(fullString[0].name, '(full)');
    assert.equal(fullString[0].value, '1234567890');

    const bytesVar = store.toVariable('b', { type: 'bytes', value: '0x01020304' });
    assert.ok(bytesVar.variablesReference && bytesVar.variablesReference > 0, 'Expected bytes to be expandable');
    assert.match(bytesVar.value, /bytes\(\d+\)/);
    const bytesDetails = store.getVariables(bytesVar.variablesReference as number);
    assert.ok(bytesDetails.some((v) => v.name === 'hex'), 'Expected bytes details to include hex');
    assert.ok(bytesDetails.some((v) => v.name === 'base64'), 'Expected bytes details to include base64');

    const addr = 'G' + 'A'.repeat(55);
    const addrVar = store.toVariable('a', addr);
    assert.equal(addrVar.type, 'address');

    console.log('Variable rendering unit tests passed');
  }

  {
    const mockServer = await startMockDebuggerServer({ evaluateDelayMs: 150 });
    const debuggerProcess = new DebuggerProcess({
      contractPath: 'mock.wasm',
      port: mockServer.port,
      spawnServer: false
    });

    await debuggerProcess.start();

    // Cancel-before-response: abort removes pending entry and ignores late responses.
    const controller = new AbortController();
    const evaluatePromise = debuggerProcess.evaluate('1', undefined, { signal: controller.signal });
    setTimeout(() => controller.abort(), 10);
    await assert.rejects(evaluatePromise, (error: any) => error?.name === 'AbortError');

    await wait(250);
    assert.equal(((debuggerProcess as any).pendingRequests as Map<number, unknown>).size, 0);
    await debuggerProcess.ping();

    // Cancel-after-timeout: timeout removes pending entry and ignores late responses.
    const timedOut = debuggerProcess.evaluate('2', undefined, { timeoutMs: 20 });
    await assert.rejects(timedOut, (error: any) => error?.name === 'TimeoutError');

    await wait(250);
    assert.equal(((debuggerProcess as any).pendingRequests as Map<number, unknown>).size, 0);
    await debuggerProcess.ping();

    await debuggerProcess.stop();
    await mockServer.close();
    console.log('Cancellation tests passed');
  }

  const emittedFiles = [
    path.join(extensionRoot, 'dist', 'extension.js'),
    path.join(extensionRoot, 'dist', 'debugAdapter.js'),
    path.join(extensionRoot, 'dist', 'cli', 'debuggerProcess.js')
  ];

  for (const file of emittedFiles) {
    assert.ok(fs.existsSync(file), `Missing compiled artifact: ${file}`);
  }

  const binaryPath = process.env.SOROBAN_DEBUG_BIN
    || path.join(repoRoot, 'target', 'debug', process.platform === 'win32' ? 'soroban-debug.exe' : 'soroban-debug');

  if (!fs.existsSync(binaryPath)) {
    console.log(`Skipping debugger smoke test because the CLI binary was not found at ${binaryPath}`);
    return;
  }

  const contractPath = path.join(repoRoot, 'tests', 'fixtures', 'wasm', 'echo.wasm');
  assert.ok(fs.existsSync(contractPath), `Missing fixture WASM: ${contractPath}`);

  const debuggerProcess = new DebuggerProcess({
    binaryPath,
    contractPath,
    entrypoint: 'echo',
    args: ['7']
  });

  await debuggerProcess.start();
  await debuggerProcess.ping();

  const sourcePath = path.join(repoRoot, 'tests', 'fixtures', 'contracts', 'echo', 'src', 'lib.rs');
  const exportedFunctions = await debuggerProcess.getContractFunctions();
  const resolvedBreakpoints = resolveSourceBreakpoints(sourcePath, [10], exportedFunctions);
  assert.equal(resolvedBreakpoints[0].verified, true, 'Expected echo breakpoint to resolve');
  assert.equal(resolvedBreakpoints[0].functionName, 'echo');

  await debuggerProcess.setBreakpoint('echo');
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
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
