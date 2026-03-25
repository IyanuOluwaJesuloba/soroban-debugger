import { ChildProcess, execFile, spawn } from 'child_process';
import * as fs from 'fs';
import * as net from 'net';
import * as path from 'path';
import { WIRE_PROTOCOL_MAX_VERSION, WIRE_PROTOCOL_MIN_VERSION } from '../dap/protocol';
import { LogManager, LogLevel, LogPhase } from '../debug/logManager';

export interface DebuggerProcessConfig {
  contractPath: string;
  snapshotPath?: string;
  entrypoint?: string;
  args?: unknown[];
  trace?: boolean;
  binaryPath?: string;
  port?: number;
  token?: string;
  requestTimeoutMs?: number;
  connectTimeoutMs?: number;
}

export type LaunchPreflightField =
  | 'binaryPath'
  | 'contractPath'
  | 'snapshotPath'
  | 'entrypoint'
  | 'args'
  | 'port'
  | 'token';

export type LaunchPreflightQuickFix =
  | 'pickBinary'
  | 'pickContract'
  | 'pickSnapshot'
  | 'openLaunchConfig'
  | 'generateLaunchConfig'
  | 'openSettings';

export interface LaunchPreflightIssue {
  field: LaunchPreflightField;
  message: string;
  expected: string;
  quickFixes: LaunchPreflightQuickFix[];
}

export interface LaunchPreflightResult {
  ok: boolean;
  issues: LaunchPreflightIssue[];
  resolvedBinaryPath: string;
}

export interface DebuggerExecutionResult {
  output: string;
  paused: boolean;
  completed: boolean;
}

export interface DebuggerInspection {
  function?: string;
  args?: string;
  stepCount: number;
  paused: boolean;
  callStack: string[];
}

export interface DebuggerContinueResult {
  completed: boolean;
  output?: string;
  paused: boolean;
}

export interface BackendBreakpointCapabilities {
  conditionalBreakpoints: boolean;
  hitConditionalBreakpoints: boolean;
  logPoints: boolean;
}

type DebugRequest =
  | { type: 'Handshake'; client_name: string; client_version: string; protocol_min: number; protocol_max: number }
  | { type: 'Authenticate'; token: string }
  | { type: 'LoadContract'; contract_path: string }
  | { type: 'Execute'; function: string; args?: string }
  | { type: 'StepIn' }
  | { type: 'Next' }
  | { type: 'StepOut' }
  | { type: 'Continue' }
  | { type: 'Inspect' }
  | { type: 'GetStorage' }
  | {
      type: 'SetBreakpoint';
      id: string;
      function: string;
      condition?: string;
      hit_condition?: string;
      log_message?: string;
    }
  | { type: 'ClearBreakpoint'; id: string }
  | { type: 'Evaluate'; expression: string; frame_id?: number }
  | { type: 'Ping' }
  | { type: 'Disconnect' }
  | { type: 'LoadSnapshot'; snapshot_path: string }
  | { type: 'GetCapabilities' };

type DebugResponse =
  | { type: 'HandshakeAck'; server_name: string; server_version: string; protocol_min: number; protocol_max: number; selected_version: number }
  | { type: 'IncompatibleProtocol'; message: string; server_name: string; server_version: string; protocol_min: number; protocol_max: number }
  | { type: 'Authenticated'; success: boolean; message: string }
  | { type: 'ContractLoaded'; size: number }
  | { type: 'ExecutionResult'; success: boolean; output: string; error?: string; paused: boolean; completed: boolean }
  | { type: 'StepResult'; paused: boolean; current_function?: string; step_count: number }
  | { type: 'ContinueResult'; completed: boolean; output?: string; error?: string; paused: boolean }
  | { type: 'InspectionResult'; function?: string; args?: string; step_count: number; paused: boolean; call_stack: string[] }
  | { type: 'StorageState'; storage_json: string }
  | { type: 'SnapshotLoaded'; summary: string }
  | { type: 'BreakpointSet'; id: string; function: string }
  | { type: 'BreakpointCleared'; id: string }
  | {
      type: 'BreakpointsList';
      breakpoints: Array<{
        id: string;
        function: string;
        condition?: string;
        hit_condition?: string;
        log_message?: string;
      }>;
    }
  | { type: 'EvaluateResult'; result: string; result_type?: string; variables_reference: number }
  | {
      type: 'Capabilities';
      breakpoints: {
        conditional_breakpoints: boolean;
        hit_conditional_breakpoints: boolean;
        log_points: boolean;
      };
    }
  | { type: 'Pong' }
  | { type: 'Disconnected' }
  | { type: 'Error'; message: string };

type DebugMessage = {
  id: number;
  request?: DebugRequest;
  response?: DebugResponse;
};

type PendingRequest = {
  resolve: (response: DebugResponse) => void;
  reject: (error: Error) => void;
};

export class DebuggerTimeoutError extends Error {
  readonly requestType: string;
  readonly timeoutMs: number;

  constructor(requestType: string, timeoutMs: number) {
    super(`Timed out waiting for debugger response to ${requestType} after ${timeoutMs}ms`);
    this.name = 'DebuggerTimeoutError';
    this.requestType = requestType;
    this.timeoutMs = timeoutMs;
  }
}

export function formatProtocolMismatchMessage(details: {
  extensionVersion: string;
  backendVersion?: string;
  backendName?: string;
  backendProtocolMin?: number;
  backendProtocolMax?: number;
  extra?: string;
}): string {
  const backendVersion = details.backendVersion || 'unknown';
  const backendName = details.backendName || 'backend';
  const backendRange = (details.backendProtocolMin !== undefined && details.backendProtocolMax !== undefined)
    ? `[${details.backendProtocolMin}..=${details.backendProtocolMax}]`
    : '(unknown range)';

  const requestedRange = `[${WIRE_PROTOCOL_MIN_VERSION}..=${WIRE_PROTOCOL_MAX_VERSION}]`;

  const lines = [
    'Incompatible debugger protocol between VS Code extension and backend.',
    `Extension version: ${details.extensionVersion} (expects protocol ${requestedRange})`,
    `${backendName} version: ${backendVersion} (supports protocol ${backendRange})`,
    details.extra ? `Details: ${details.extra}` : undefined,
    'Remediation: upgrade the older component so both support at least one common protocol version.'
  ].filter(Boolean);

  return lines.join('\n');
}

export class DebuggerProcess {
  private childProcess: ChildProcess | null = null;
  private socket: net.Socket | null = null;
  private buffer = '';
  private requestId = 0;
  private pendingRequests = new Map<number, PendingRequest>();
  private config: DebuggerProcessConfig;
  private logManager: LogManager | undefined;
  private port: number | null = null;
  private negotiatedProtocolVersion: number | null = null;
  private defaultRequestTimeoutMs: number;
  private defaultConnectTimeoutMs: number;

  constructor(config: DebuggerProcessConfig, logManager?: LogManager) {
    this.config = config;
    this.logManager = logManager;

    const envRequestTimeout = Number(process.env.SOROBAN_DEBUG_REQUEST_TIMEOUT_MS);
    const envConnectTimeout = Number(process.env.SOROBAN_DEBUG_CONNECT_TIMEOUT_MS);

    this.defaultRequestTimeoutMs = Number.isFinite(config.requestTimeoutMs)
      ? Number(config.requestTimeoutMs)
      : (Number.isFinite(envRequestTimeout) ? envRequestTimeout : 30_000);

    this.defaultConnectTimeoutMs = Number.isFinite(config.connectTimeoutMs)
      ? Number(config.connectTimeoutMs)
      : (Number.isFinite(envConnectTimeout) ? envConnectTimeout : 10_000);
  }

  async start(): Promise<void> {
    if (this.childProcess || this.socket) {
      return;
    }

    this.logManager?.log(LogLevel.Info, LogPhase.Lifecycle, 'Starting debugger backend process...');

    const binaryPath = resolveDebuggerBinaryPath(this.config);
    const port = this.config.port ?? await this.findAvailablePort();
    this.port = port;

    this.logManager?.log(LogLevel.Info, LogPhase.Spawn, `Spawning backend: ${binaryPath} server --port ${port}`);

    const child = spawn(binaryPath, this.buildArgs(port), {
      stdio: ['ignore', 'pipe', 'pipe'],
      env: {
        ...process.env,
        ...(this.config.trace ? { RUST_LOG: 'debug' } : {})
      }
    });
    this.childProcess = child;

    try {
      child.once('exit', () => {
        this.rejectPendingRequests(new Error('Debugger server exited'));
        this.socket?.destroy();
        this.socket = null;
      });

      await this.waitForServer(port);
      this.logManager?.log(LogLevel.Info, LogPhase.Connect, `Connecting to debugger server on port ${port}...`);
      await this.connect(port);
      this.logManager?.log(LogLevel.Info, LogPhase.Connect, 'Connection established. Negotiating protocol...');
      await this.negotiateProtocol();
      this.logManager?.log(LogLevel.Info, LogPhase.Connect, `Protocol negotiated: ${this.negotiatedProtocolVersion || 'unknown'}`);

      if (this.config.token) {
        this.logManager?.log(LogLevel.Info, LogPhase.Auth, 'Authenticating with token...');
        const response = await this.sendRequest({
          type: 'Authenticate',
          token: this.config.token
        });
        this.expectResponse(response, 'Authenticated');
        if (!response.success) {
          throw new Error(response.message);
        }
      }

      if (this.config.snapshotPath) {
        this.logManager?.log(LogLevel.Info, LogPhase.Load, `Loading snapshot: ${this.config.snapshotPath}`);
        const response = await this.sendRequest({
          type: 'LoadSnapshot',
          snapshot_path: this.config.snapshotPath
        });
        this.expectResponse(response, 'SnapshotLoaded');
      }

      this.logManager?.log(LogLevel.Info, LogPhase.Load, `Loading contract: ${this.config.contractPath}`);
      const contractResponse = await this.sendRequest({
        type: 'LoadContract',
        contract_path: this.config.contractPath
      });
      this.expectResponse(contractResponse, 'ContractLoaded');
    } catch (error) {
      await this.stop().catch(() => undefined);
      throw error;
    }
  }

  async execute(): Promise<DebuggerExecutionResult> {
    const response = await this.sendRequest({
      type: 'Execute',
      function: this.config.entrypoint || 'main',
      args: this.config.args && this.config.args.length > 0
        ? JSON.stringify(this.config.args)
        : undefined
    });
    this.expectResponse(response, 'ExecutionResult');

    if (!response.success) {
      throw new Error(response.error || 'Execution failed');
    }

    return {
      output: response.output,
      paused: response.paused,
      completed: response.completed
    };
  }

  async stepIn(): Promise<{ paused: boolean; current_function?: string; step_count: number }> {
    const response = await this.sendRequest({ type: 'StepIn' });
    this.expectResponse(response, 'StepResult');
    return response as any;
  }

  async next(): Promise<{ paused: boolean; current_function?: string; step_count: number }> {
    const response = await this.sendRequest({ type: 'Next' });
    this.expectResponse(response, 'StepResult');
    return response as any;
  }

  async stepOut(): Promise<{ paused: boolean; current_function?: string; step_count: number }> {
    const response = await this.sendRequest({ type: 'StepOut' });
    this.expectResponse(response, 'StepResult');
    return response as any;
  }

  async continueExecution(): Promise<DebuggerContinueResult> {
    const response = await this.sendRequest({ type: 'Continue' });
    this.expectResponse(response, 'ContinueResult');
    if (response.error) {
      throw new Error(response.error);
    }
    return {
      completed: response.completed,
      output: response.output,
      paused: response.paused
    };
  }

  async inspect(): Promise<DebuggerInspection> {
    const response = await this.sendRequest({ type: 'Inspect' });
    this.expectResponse(response, 'InspectionResult');
    return {
      function: response.function,
      args: response.args,
      stepCount: response.step_count,
      paused: response.paused,
      callStack: response.call_stack
    };
  }

  async getStorage(): Promise<Record<string, unknown>> {
    const response = await this.sendRequest({ type: 'GetStorage' });
    this.expectResponse(response, 'StorageState');
    const parsed = JSON.parse(response.storage_json);
    if (parsed && typeof parsed === 'object') {
      return parsed as Record<string, unknown>;
    }
    return {};
  }

  async ping(): Promise<void> {
    const response = await this.sendRequest({ type: 'Ping' });
    this.expectResponse(response, 'Pong');
  }

  async getCapabilities(): Promise<BackendBreakpointCapabilities> {
    const response = await this.sendRequest({ type: 'GetCapabilities' });
    this.expectResponse(response, 'Capabilities');
    return {
      conditionalBreakpoints: response.breakpoints.conditional_breakpoints,
      hitConditionalBreakpoints: response.breakpoints.hit_conditional_breakpoints,
      logPoints: response.breakpoints.log_points
    };
  }

  async setBreakpoint(breakpoint: {
    id: string;
    functionName: string;
    condition?: string;
    hitCondition?: string;
    logMessage?: string;
  }): Promise<void> {
    const response = await this.sendRequest({
      type: 'SetBreakpoint',
      id: breakpoint.id,
      function: breakpoint.functionName,
      condition: breakpoint.condition,
      hit_condition: breakpoint.hitCondition,
      log_message: breakpoint.logMessage
    });
    this.expectResponse(response, 'BreakpointSet');
  }

  async clearBreakpoint(breakpointId: string): Promise<void> {
    const response = await this.sendRequest({
      type: 'ClearBreakpoint',
      id: breakpointId
    });
    this.expectResponse(response, 'BreakpointCleared');
  }

  async evaluate(expression: string, frameId?: number): Promise<{ result: string; type?: string; variablesReference: number }> {
    const response = await this.sendRequest({
      type: 'Evaluate',
      expression,
      frame_id: frameId
    });
    this.expectResponse(response, 'EvaluateResult');
    return {
      result: response.result,
      type: response.result_type,
      variablesReference: response.variables_reference
    };
  }

  async getContractFunctions(): Promise<Set<string>> {
    const binaryPath = resolveDebuggerBinaryPath(this.config);

    const output = await new Promise<string>((resolve, reject) => {
      const child = execFile(
        binaryPath,
        ['inspect', '--contract', this.config.contractPath, '--functions'],
        { env: process.env },
        (error, stdout, stderr) => {
          clearTimeout(timer);
          if (error) {
            reject(new Error(stderr || stdout || String(error)));
            return;
          }
          resolve(stdout);
        }
      );

      const timer = setTimeout(() => {
        child.kill();
        reject(new DebuggerTimeoutError('InspectFunctions', this.defaultRequestTimeoutMs));
      }, this.defaultRequestTimeoutMs);
    });

    const functions = new Set<string>();
    for (const line of output.split(/\r?\n/)) {
      const match = line.match(/^\s*([A-Za-z_][A-Za-z0-9_]*)\(/);
      if (match) {
        functions.add(match[1]);
      }
    }

    return functions;
  }

  async stop(): Promise<void> {
    try {
      if (this.socket && !this.socket.destroyed) {
        await this.sendRequest({ type: 'Disconnect' }).catch(() => undefined);
      }
    } finally {
      this.socket?.destroy();
      this.socket = null;
    }

    if (!this.childProcess) {
      return;
    }

    if (this.childProcess.killed) {
      this.childProcess = null;
      return;
    }

    await new Promise<void>((resolve) => {
      if (!this.childProcess) {
        resolve();
        return;
      }

      const child = this.childProcess;
      const timeout = setTimeout(() => {
        if (!child.killed) {
          child.kill('SIGKILL');
        }
      }, 5000);

      child.once('exit', () => {
        clearTimeout(timeout);
        resolve();
      });
      child.kill('SIGTERM');
    });

    this.childProcess = null;
  }

  getOutputStream() {
    return this.childProcess?.stdout;
  }

  getErrorStream() {
    return this.childProcess?.stderr;
  }

  private buildArgs(port: number): string[] {
    const args = ['server', '--port', String(port)];

    if (this.config.token) {
      args.push('--token', this.config.token);
    }

    return args;
  }

  isRunning(): boolean {
    return this.childProcess !== null && this.socket !== null && !this.socket.destroyed;
  }

  private async findAvailablePort(): Promise<number> {
    return await new Promise<number>((resolve, reject) => {
      const server = net.createServer();
      server.listen(0, '127.0.0.1', () => {
        const address = server.address();
        if (!address || typeof address === 'string') {
          reject(new Error('Failed to determine an available port'));
          return;
        }

        const port = address.port;
        server.close((error) => {
          if (error) {
            reject(error);
            return;
          }
          resolve(port);
        });
      });
      server.on('error', reject);
    });
  }

  private async waitForServer(port: number): Promise<void> {
    const deadline = Date.now() + this.defaultConnectTimeoutMs;

    while (Date.now() < deadline) {
      if (this.childProcess && this.childProcess.exitCode !== null) {
        throw new Error(`Debugger server exited with code ${this.childProcess.exitCode}`);
      }

      if (await this.canConnect(port)) {
        return;
      }

      await new Promise(resolve => setTimeout(resolve, 100));
    }

    throw new Error(`Timed out waiting for debugger server on port ${port}`);
  }

  private async canConnect(port: number): Promise<boolean> {
    return await new Promise<boolean>((resolve) => {
      const socket = net.createConnection({ host: '127.0.0.1', port }, () => {
        socket.destroy();
        resolve(true);
      });

      socket.on('error', () => {
        socket.destroy();
        resolve(false);
      });
    });
  }

  private async connect(port: number): Promise<void> {
    await new Promise<void>((resolve, reject) => {
      const socket = net.createConnection({ host: '127.0.0.1', port }, () => {
        this.socket = socket;
        resolve();
      });

      socket.setEncoding('utf8');
      socket.on('data', (chunk: string) => {
        this.buffer += chunk;
        this.consumeMessages();
      });
      socket.on('error', reject);
      socket.on('close', () => {
        this.rejectPendingRequests(new Error('Debugger connection closed'));
        this.socket = null;
      });
    });
  }

  private consumeMessages(): void {
    while (true) {
      const newlineIndex = this.buffer.indexOf('\n');
      if (newlineIndex === -1) {
        return;
      }

      const line = this.buffer.slice(0, newlineIndex).trim();
      this.buffer = this.buffer.slice(newlineIndex + 1);

      if (!line) {
        continue;
      }

      let message: DebugMessage;
      try {
        message = JSON.parse(line) as DebugMessage;
      } catch {
        continue;
      }
      const pending = this.pendingRequests.get(message.id);
      if (!pending || !message.response) {
        continue;
      }

      this.pendingRequests.delete(message.id);
      this.logManager?.log(LogLevel.Debug, LogPhase.Connect, `Backend response [${message.id}]: ${JSON.stringify(message.response)}`);
      pending.resolve(message.response);
    }
  }

  private async sendRequest(
    request: DebugRequest,
    options: { timeoutMs?: number } = {}
  ): Promise<DebugResponse> {
    if (!this.socket) {
      throw new Error('Debugger connection is not established');
    }

    this.requestId += 1;
    const id = this.requestId;
    const message: DebugMessage = { id, request };

    const responsePromise = new Promise<DebugResponse>((resolve, reject) => {
      const timeoutMs = options.timeoutMs ?? this.defaultRequestTimeoutMs;
      const timer = setTimeout(() => {
        this.pendingRequests.delete(id);
        reject(new DebuggerTimeoutError(request.type, timeoutMs));
      }, timeoutMs);

      this.pendingRequests.set(id, {
        resolve: (response) => {
          clearTimeout(timer);
          resolve(response);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        }
      });
    });

    this.logManager?.log(LogLevel.Debug, LogPhase.Connect, `Backend request [${id}]: ${JSON.stringify(request)}`);
    this.socket.write(`${JSON.stringify(message)}\n`);
    const response = await responsePromise;
    if (response.type === 'Error') {
      throw new Error(response.message);
    }
    return response;
  }

  private getExtensionVersion(): string {
    try {
      const packageJsonPath = path.resolve(__dirname, '..', '..', 'package.json');
      const pkg = JSON.parse(fs.readFileSync(packageJsonPath, 'utf8')) as { version?: string };
      return pkg.version || 'unknown';
    } catch {
      return 'unknown';
    }
  }

  private async negotiateProtocol(): Promise<void> {
    const extensionVersion = this.getExtensionVersion();

    let response: DebugResponse;
    try {
      response = await this.sendRequest({
        type: 'Handshake',
        client_name: 'vscode-extension',
        client_version: extensionVersion,
        protocol_min: WIRE_PROTOCOL_MIN_VERSION,
        protocol_max: WIRE_PROTOCOL_MAX_VERSION
      }, { timeoutMs: 2_500 });
    } catch (error) {
      throw new Error(formatProtocolMismatchMessage({
        extensionVersion,
        extra: String(error)
      }));
    }

    if (response.type === 'HandshakeAck') {
      this.negotiatedProtocolVersion = response.selected_version;
      return;
    }

    if (response.type === 'IncompatibleProtocol') {
      throw new Error(formatProtocolMismatchMessage({
        extensionVersion,
        backendName: response.server_name,
        backendVersion: response.server_version,
        backendProtocolMin: response.protocol_min,
        backendProtocolMax: response.protocol_max,
        extra: response.message
      }));
    }

    throw new Error(formatProtocolMismatchMessage({
      extensionVersion,
      extra: `Unexpected handshake response: ${response.type}`
    }));
  }

  private rejectPendingRequests(error: Error): void {
    for (const pending of this.pendingRequests.values()) {
      pending.reject(error);
    }
    this.pendingRequests.clear();
  }

  private expectResponse<T extends DebugResponse['type']>(
    response: DebugResponse,
    type: T
  ): asserts response is Extract<DebugResponse, { type: T }> {
    if (response.type !== type) {
      throw new Error(`Unexpected debugger response: expected ${type}, got ${response.type}`);
    }
  }
}

function pushFileIssue(
  issues: LaunchPreflightIssue[],
  field: 'binaryPath' | 'contractPath' | 'snapshotPath',
  filePath: string | undefined,
  expected: string,
  quickFixes: LaunchPreflightQuickFix[]
): void {
  if (!filePath || filePath.trim().length === 0) {
    issues.push({
      field,
      message: `Launch config field '${field}' must point to ${expected}.`,
      expected,
      quickFixes
    });
    return;
  }

  if (field === 'binaryPath' && isCommandOnPath(filePath)) {
    return;
  }

  try {
    const stat = fs.statSync(filePath);
    if (!stat.isFile()) {
      issues.push({
        field,
        message: `Launch config field '${field}' points to '${filePath}', but that path is not a file.`,
        expected,
        quickFixes
      });
      return;
    }

    fs.accessSync(filePath, fs.constants.R_OK);
  } catch {
    issues.push({
      field,
      message: `Launch config field '${field}' points to '${filePath}', but the file does not exist or is not readable.`,
      expected,
      quickFixes
    });
  }
}

function isCommandOnPath(command: string): boolean {
  if (path.isAbsolute(command) || command.includes(path.sep) || command.includes('/')) {
    return false;
  }

  const pathValue = process.env.PATH;
  if (!pathValue) {
    return false;
  }

  const extensions = process.platform === 'win32'
    ? (process.env.PATHEXT || '.EXE;.CMD;.BAT;.COM')
        .split(';')
        .filter((ext) => ext.length > 0)
    : [''];

  for (const directory of pathValue.split(path.delimiter)) {
    for (const extension of extensions) {
      const candidate = path.join(directory, command.endsWith(extension) ? command : `${command}${extension}`);
      if (fs.existsSync(candidate)) {
        return true;
      }
    }
  }

  return false;
}

function validateArgs(args: unknown): LaunchPreflightIssue | undefined {
  if (!Array.isArray(args)) {
    return {
      field: 'args',
      message: `Launch config field 'args' must be an array; received ${describeValue(args)}.`,
      expected: 'A JSON array such as [] or ["alice", 10].',
      quickFixes: ['openLaunchConfig', 'generateLaunchConfig']
    };
  }

  const seen = new Set<unknown>();
  const invalidPath = findNonSerializableValue(args, '$', seen);
  if (invalidPath) {
    return {
      field: 'args',
      message: `Launch config field 'args' contains a non-JSON-serializable value at ${invalidPath}.`,
      expected: 'Only JSON-serializable values: strings, numbers, booleans, null, arrays, and objects.',
      quickFixes: ['openLaunchConfig', 'generateLaunchConfig']
    };
  }

  return undefined;
}

function findNonSerializableValue(value: unknown, pathLabel: string, seen: Set<unknown>): string | undefined {
  if (
    value === null ||
    typeof value === 'string' ||
    typeof value === 'boolean'
  ) {
    return undefined;
  }

  if (typeof value === 'number') {
    return Number.isFinite(value) ? undefined : pathLabel;
  }

  if (
    typeof value === 'undefined' ||
    typeof value === 'function' ||
    typeof value === 'symbol' ||
    typeof value === 'bigint'
  ) {
    return pathLabel;
  }

  if (Array.isArray(value)) {
    if (seen.has(value)) {
      return pathLabel;
    }
    seen.add(value);
    for (let index = 0; index < value.length; index += 1) {
      const found = findNonSerializableValue(value[index], `${pathLabel}[${index}]`, seen);
      if (found) {
        return found;
      }
    }
    seen.delete(value);
    return undefined;
  }

  if (typeof value === 'object') {
    const record = value as Record<string, unknown>;
    if (seen.has(record)) {
      return pathLabel;
    }
    seen.add(record);
    for (const [key, item] of Object.entries(record)) {
      const found = findNonSerializableValue(item, `${pathLabel}.${key}`, seen);
      if (found) {
        return found;
      }
    }
    seen.delete(record);
    return undefined;
  }

  return pathLabel;
}

function describeValue(value: unknown): string {
  if (Array.isArray(value)) {
    return 'an array';
  }
  if (value === null) {
    return 'null';
  }
  return typeof value;
}

async function isPortAvailable(port: number): Promise<boolean> {
  return await new Promise<boolean>((resolve) => {
    const server = net.createServer();
    server.once('error', () => {
      server.close();
      resolve(false);
    });
    server.once('listening', () => {
      server.close(() => resolve(true));
    });
    server.listen(port, '127.0.0.1');
  });
}
