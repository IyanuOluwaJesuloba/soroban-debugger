import {
  DebugSession,
  InitializedEvent,
  StoppedEvent,
  ExitedEvent} from '@vscode/debugadapter';
import { DebugProtocol } from '@vscode/debugprotocol';
import * as readline from 'readline';
import { DebuggerProcess, DebuggerProcessConfig } from '../cli/debuggerProcess';
import { BreakpointCapabilities, BreakpointLocation, DebuggerState, Variable } from './protocol';
import { ResolvedBreakpoint, resolveSourceBreakpoints } from './sourceBreakpoints';
import { LogOutputEvent, LogLevel } from '@vscode/debugadapter/lib/logger';

type LaunchRequestArgs = DebugProtocol.LaunchRequestArguments & DebuggerProcessConfig;

export class SorobanDebugSession extends DebugSession {
  private debuggerProcess: DebuggerProcess | null = null;
  private state: DebuggerState = {
    isRunning: false,
    isPaused: false,
    breakpoints: new Map(),
    callStack: [],
    variables: []
  };
  private variableHandles = new Map<number, any>();
  private nextVarHandle = 1;
  private threadId = 1;
  private outputReaders: readline.Interface[] = [];
  private hasExecuted = false;
  private exportedFunctions = new Set<string>();
  private sourceFunctionBreakpoints = new Map<string, Set<string>>();
  private backendCapabilities: BreakpointCapabilities = {
    conditionalBreakpoints: true,
    hitConditionalBreakpoints: true,
    logPoints: true
  };

  protected initializeRequest(
    response: DebugProtocol.InitializeResponse,
    args: DebugProtocol.InitializeRequestArguments
  ): void {
    response.body = response.body || {};
    response.body.supportsConfigurationDoneRequest = true;
    response.body.supportsEvaluateForHovers = true;
    response.body.supportsSetVariable = false;
    response.body.supportsSetExpression = false;
    response.body.supportsConditionalBreakpoints = true;
    response.body.supportsHitConditionalBreakpoints = true;
    response.body.supportsLogPoints = true;

    this.sendResponse(response);
    this.sendEvent(new InitializedEvent());
  }

  protected async launchRequest(
    response: DebugProtocol.LaunchResponse,
    args: LaunchRequestArgs
  ): Promise<void> {
    try {
      const preflight = await validateLaunchConfig(args);
      if (!preflight.ok) {
        const issue = preflight.issues[0];
        throw new Error(`${issue.message} Expected: ${issue.expected}`);
      }

      this.debuggerProcess = new DebuggerProcess({
        contractPath: args.contractPath,
        snapshotPath: args.snapshotPath,
        entrypoint: args.entrypoint || 'main',
        args: args.args || [],
        trace: args.trace || false,
        binaryPath: args.binaryPath,
        port: args.port,
        token: args.token,
        requestTimeoutMs: args.requestTimeoutMs,
        connectTimeoutMs: args.connectTimeoutMs
      });

      await this.debuggerProcess.start();
      this.state.isRunning = true;
      this.state.isPaused = false;
      this.hasExecuted = false;
      this.variableHandles.clear();
      this.nextVarHandle = 1;
      this.exportedFunctions = await this.debuggerProcess.getContractFunctions();
      this.backendCapabilities = await this.debuggerProcess.getCapabilities().catch(() => ({
        conditionalBreakpoints: false,
        hitConditionalBreakpoints: false,
        logPoints: false
      }));

      this.attachProcessListeners();
      this.sendResponse(response);
    } catch (error) {
      const message = error instanceof DebuggerTimeoutError
        ? `Failed to launch debugger (timeout): ${error.message}\n\nNext steps: ensure the backend process is running, reachable, and not stalled; then retry the session.`
        : `Failed to launch debugger: ${error}`;
      this.sendErrorResponse(response, {
        id: 1001,
        format: message,
        showUser: true
      });
    }
  }

  protected async setBreakpointsRequest(
    response: DebugProtocol.SetBreakpointsResponse,
    args: DebugProtocol.SetBreakpointsArguments
  ): Promise<void> {
    const source = args.source.path || args.source.name || '';
    const breakpoints = args.breakpoints || [];
    const lines = breakpoints.map((bp) => bp.line);

    try {
      const resolved: ResolvedBreakpoint[] = this.debuggerProcess && source
        ? resolveSourceBreakpoints(source, lines, this.exportedFunctions)
        : lines.map((line) => ({
            line,
            verified: false,
            message: 'Debugger is not launched or source path is unavailable'
          }));

      const managedBreakpoints: BreakpointLocation[] = breakpoints.map((bp, index) => {
        const match = resolved.find((resolvedBreakpoint) => resolvedBreakpoint.line === bp.line);
        return {
          id: `${source}:${bp.line}:${bp.column ?? 1}:${index}`,
          source,
          line: bp.line,
          column: bp.column,
          functionName: match?.functionName,
          condition: bp.condition,
          hitCondition: bp.hitCondition,
          logMessage: bp.logMessage
        };
      });

      const syncErrors = await this.syncSourceBreakpoints(
        source,
        managedBreakpoints.filter((bp) => {
          const match = resolved.find((resolvedBreakpoint) => resolvedBreakpoint.line === bp.line);
          return Boolean(match?.verified && bp.functionName);
        })
      );

      this.state.breakpoints.set(source, managedBreakpoints);
      this.sourceFunctionBreakpoints.set(
        source,
        new Set(
          managedBreakpoints
            .filter((bp) => Boolean(bp.functionName) && !syncErrors.has(bp.id))
            .map((bp) => bp.functionName as string)
        )
      );

      response.body = {
        breakpoints: breakpoints.map((bp) => {
          const match = resolved.find((resolvedBreakpoint) => resolvedBreakpoint.line === bp.line);
          const managed = managedBreakpoints.find((candidate) => candidate.line === bp.line);
          const capabilityMessages = this.describeCapabilityFallback(bp);
          const syncMessage = managed ? syncErrors.get(managed.id) : undefined;
          return {
            verified: (match?.verified ?? false) && !syncMessage,
            line: bp.line,
            column: bp.column,
            source: args.source,
            message: [match?.message, syncMessage, capabilityMessages].filter(Boolean).join(' ')
          };
        })
      };

      this.sendResponse(response);
    } catch (error) {
      this.sendErrorResponse(response, {
        id: 1003,
        format: `Failed to resolve breakpoints: ${error}`,
        showUser: true
      });
    }
  }

  protected async stackTraceRequest(
    response: DebugProtocol.StackTraceResponse,
    args: DebugProtocol.StackTraceArguments
  ): Promise<void> {
    const stackFrames = this.state.callStack || [];

    response.body = {
      stackFrames: stackFrames.slice(0, 50).map(frame => ({
        id: frame.id,
        name: frame.name,
        source: {
          name: frame.source,
          path: frame.source
        },
        line: frame.line,
        column: frame.column,
        instructionPointerReference: frame.instructionPointerReference
      }))
    };

    this.sendResponse(response);
  }

  protected async scopesRequest(
    response: DebugProtocol.ScopesResponse,
    args: DebugProtocol.ScopesArguments
  ): Promise<void> {
    // Rebuild handles each time to reflect the latest paused state.
    this.variableHandles.clear();
    this.nextVarHandle = 1;

    const scopes: DebugProtocol.Scope[] = [];

    const argsRef = this.nextVarHandle++;
    this.variableHandles.set(argsRef, this.argsToVariables(this.state.args));
    scopes.push({
      name: 'Arguments',
      variablesReference: argsRef,
      expensive: false
    });

    if (this.state.variables && this.state.variables.length > 0) {
      const variablesRef = this.nextVarHandle++;
      this.variableHandles.set(variablesRef, this.state.variables);

      scopes.push({
        name: 'Storage',
        variablesReference: variablesRef,
        expensive: false
      });
    }

    response.body = { scopes };
    this.sendResponse(response);
  }

  protected async variablesRequest(
    response: DebugProtocol.VariablesResponse,
    args: DebugProtocol.VariablesArguments
  ): Promise<void> {
    const variables = this.variableHandles.get(args.variablesReference) || [];

    response.body = {
      variables: variables.map((v: Variable) => ({
        name: v.name,
        value: v.value,
        type: v.type,
        variablesReference: v.variablesReference || 0
      }))
    };

    this.sendResponse(response);
  }

  protected async evaluateRequest(
    response: DebugProtocol.EvaluateResponse,
    args: DebugProtocol.EvaluateArguments
  ): Promise<void> {
    const expression = (args.expression || '').trim();

    try {
      if (this.debuggerProcess && this.state.isPaused) {
        await this.refreshState();
      }

      if (expression === 'args' || expression === 'Arguments') {
        response.body = {
          result: this.state.args ?? '(none)',
          variablesReference: 0
        };
        this.sendResponse(response);
        return;
      }

      if (expression === 'storage' || expression === 'Storage') {
        const storageObject = Object.fromEntries(
          (this.state.variables || []).map((v) => [v.name, v.value])
        );
        response.body = {
          result: JSON.stringify(storageObject),
          variablesReference: 0
        };
        this.sendResponse(response);
        return;
      }

      if (expression.startsWith('storage.')) {
        const key = expression.slice('storage.'.length);
        const match = (this.state.variables || []).find((v) => v.name === key);
        if (!match) {
          throw new Error(`Unknown storage key: ${key}`);
        }
        response.body = {
          result: match.value,
          variablesReference: 0
        };
        this.sendResponse(response);
        return;
      }

      throw new Error('Unsupported expression. Try `args`, `storage`, or `storage.<key>`.');
    } catch (error) {
      if (error instanceof DebuggerTimeoutError) {
        this.sendErrorResponse(response, {
          id: 1010,
          format:
            `Evaluate timed out (${error.requestType}). The backend may be stalled.\n\n` +
            `Next steps: restart the debug session; if it persists, verify the backend binary and connectivity.`,
          showUser: true
        });
        return;
      }

      this.sendErrorResponse(response, {
        id: 1010,
        format: `Evaluate failed: ${error}`,
        showUser: false
      });
    }
  }

  protected async continueRequest(
    response: DebugProtocol.ContinueResponse,
    args: DebugProtocol.ContinueArguments
  ): Promise<void> {
    try {
      if (!this.debuggerProcess) {
        throw new Error('Debugger process is not running');
      }

      response.body = { allThreadsContinued: true };
      this.sendResponse(response);

      if (!this.hasExecuted) {
        await this.runExecution('step');
        return;
      }

      const result = await this.debuggerProcess.continueExecution();
      if (result.output) {
        this.sendEvent(new LogOutputEvent(`Result: ${result.output}\n`, LogLevel.Log));
      }

      if (result.paused) {
        await this.refreshState();
        this.state.isPaused = true;
        this.sendEvent(new StoppedEvent('breakpoint', this.threadId));
        return;
      }

      this.sendEvent(new ExitedEvent(0));
      await this.stop();
    } catch (error) {
      if (error instanceof DebuggerTimeoutError) {
        this.sendEvent(new LogOutputEvent(
          `Debugger request timed out (${error.requestType}). The backend may be stalled or the connection is unhealthy.\n` +
          `Next steps: restart the debug session; if it persists, verify the backend binary and network connectivity.\n`,
          LogLevel.Error
        ));
        this.sendEvent(new ExitedEvent(1));
        await this.stop();
        return;
      }

      this.sendErrorResponse(response, {
        id: 1002,
        format: `Continue failed: ${error}`,
        showUser: true
      });
    }
  }

  protected async nextRequest(
    response: DebugProtocol.NextResponse,
    args: DebugProtocol.NextArguments
  ): Promise<void> {
    if (!this.debuggerProcess) {
      this.sendResponse(response);
      return;
    }

    try {
      const result = await this.debuggerProcess.sendCommand({
        type: 'StepOverLine'
      });
      
      this.sendResponse(response);

      if (result && result.paused) {
        this.state.isPaused = true;
        this.sendEvent(new StoppedEvent('step', this.threadId));
      }
    } catch (e) {
      this.sendResponse(response);
    }
    await this.stepOnce(response, 'next');
  }

  protected async stepInRequest(
    response: DebugProtocol.StepInResponse,
    args: DebugProtocol.StepInArguments
  ): Promise<void> {
    await this.stepOnce(response, 'step in');
  }

  protected async stepOutRequest(
    response: DebugProtocol.StepOutResponse,
    args: DebugProtocol.StepOutArguments
  ): Promise<void> {
    await this.stepOnce(response, 'step out');
  }

  protected async threadRequest(
    response: DebugProtocol.ThreadsResponse
  ): Promise<void> {
    response.body = {
      threads: [{
        id: this.threadId,
        name: 'Main Thread'
      }]
    };
    this.sendResponse(response);
  }

  protected async configurationDoneRequest(
    response: DebugProtocol.ConfigurationDoneResponse,
    args: DebugProtocol.ConfigurationDoneArguments
  ): Promise<void> {
    try {
      if (this.debuggerProcess) {
        await this.refreshState();
        this.state.isPaused = true;
        this.sendEvent(new StoppedEvent('entry', this.threadId));
      }
      this.sendResponse(response);
    } catch (error) {
      if (error instanceof DebuggerTimeoutError) {
        this.sendEvent(new LogOutputEvent(
          `[timeout] configurationDone refresh timed out (${error.requestType}).\n` +
          `Next steps: restart the debug session.\n`,
          LogLevel.Error
        ));
        this.sendEvent(new ExitedEvent(1));
        await this.stop();
        this.sendResponse(response);
        return;
      }

      this.sendErrorResponse(response, {
        id: 1009,
        format: `Configuration failed: ${error}`,
        showUser: true
      });
    }
  }

  protected async evaluateRequest(
    response: DebugProtocol.EvaluateResponse,
    args: DebugProtocol.EvaluateArguments
  ): Promise<void> {
    if (!this.debuggerProcess || !this.state.isPaused) {
      this.sendErrorResponse(response, {
        id: 1004,
        format: 'Evaluation is only available when the debugger is paused',
        showUser: true
      });
      return;
    }

    try {
      const result = await this.debuggerProcess.evaluate(args.expression, args.frameId);
      response.body = {
        result: result.result,
        type: result.type,
        variablesReference: result.variablesReference
      };
      this.sendResponse(response);
    } catch (error) {
      this.sendErrorResponse(response, {
        id: 1005,
        format: `${error}`,
        showUser: false
      });
    }
  }

  protected async disconnectRequest(
    response: DebugProtocol.DisconnectResponse,
    args: DebugProtocol.DisconnectArguments
  ): Promise<void> {
    await this.stop();
    this.sendResponse(response);
  }

  private attachProcessListeners(): void {
    if (!this.debuggerProcess) return;

    const stdout = this.debuggerProcess.getOutputStream();
    if (stdout) {
      const reader = readline.createInterface({
        input: stdout,
        crlfDelay: Infinity
      });

      reader.on('line', (line: string) => {
        this.sendEvent(new LogOutputEvent(line + '\n', LogLevel.Log));
      });
      this.outputReaders.push(reader);
    }

    const stderr = this.debuggerProcess.getErrorStream();
    if (stderr) {
      const reader = readline.createInterface({
        input: stderr,
        crlfDelay: Infinity
      });

      reader.on('line', (line: string) => {
        this.sendEvent(new LogOutputEvent(line + '\n', LogLevel.Error));
      });
      this.outputReaders.push(reader);
    }
  }

  private async runExecution(reason: 'step' | 'entry' | 'breakpoint' | 'pause'): Promise<void> {
    if (!this.debuggerProcess) {
      throw new Error('Debugger process is not running');
    }

    const result = await this.debuggerProcess.execute();
    this.hasExecuted = true;
    await this.refreshState();
    if (result.output) {
      this.sendEvent(new LogOutputEvent(`Result: ${result.output}\n`, LogLevel.Log));
    }

    if (result.paused) {
      this.state.isPaused = true;
      this.sendEvent(new StoppedEvent('breakpoint', this.threadId));
      return;
    }

    this.state.isPaused = false;
    this.sendEvent(new ExitedEvent(0));
    await this.stop();
  }

  private async stepOnce(
    response:
      | DebugProtocol.NextResponse
      | DebugProtocol.StepInResponse
      | DebugProtocol.StepOutResponse,
    label: string
  ): Promise<void> {
    try {
      if (!this.debuggerProcess) {
        throw new Error('Debugger process is not running');
      }

      this.sendResponse(response);

      if (!this.hasExecuted) {
        await this.runExecution('step');
        return;
      }

      let result;
      if (label === 'next') {
        result = await this.debuggerProcess.next();
      } else if (label === 'step in') {
        result = await this.debuggerProcess.stepIn();
      } else if (label === 'step out') {
        result = await this.debuggerProcess.stepOut();
      } else {
        result = await this.debuggerProcess.stepIn(); // Fallback
      }

      if (result.paused) {
        await this.refreshState();
        this.state.isPaused = true;
        this.sendEvent(new StoppedEvent('step', this.threadId));
        return;
      }

      this.sendEvent(new ExitedEvent(0));
      await this.stop();
    } catch (error) {
      if (error instanceof DebuggerTimeoutError) {
        this.sendEvent(new LogOutputEvent(
          `${label} timed out (${error.requestType}). The backend may be stalled.\n` +
          `Next steps: restart the debug session.\n`,
          LogLevel.Error
        ));
        this.sendEvent(new ExitedEvent(1));
        await this.stop();
        return;
      }

      this.sendEvent(new LogOutputEvent(`${label} failed: ${error}\n`, LogLevel.Error));
    }
  }

  private async syncSourceBreakpoints(
    source: string,
    nextBreakpoints: BreakpointLocation[]
  ): Promise<Map<string, string>> {
    if (!this.debuggerProcess) {
      return new Map();
    }

    const previousBreakpoints = this.state.breakpoints.get(source) || [];
    const errors = new Map<string, string>();

    for (const breakpoint of previousBreakpoints) {
      await this.debuggerProcess.clearBreakpoint(breakpoint.id);
    }

    for (const breakpoint of nextBreakpoints) {
      try {
        await this.debuggerProcess.setBreakpoint({
          id: breakpoint.id,
          functionName: breakpoint.functionName as string,
          condition: breakpoint.condition,
          hitCondition: breakpoint.hitCondition,
          logMessage: breakpoint.logMessage
        });
      } catch (error) {
        errors.set(
          breakpoint.id,
          error instanceof Error ? error.message : String(error)
        );
      }
    }

    return errors;
  }

  private async refreshState(): Promise<void> {
    if (!this.debuggerProcess) {
      return;
    }

    const [inspection, storage] = await Promise.all([
      this.debuggerProcess.inspect(),
      this.debuggerProcess.getStorage()
    ]);

    this.state.callStack = inspection.callStack.map((frame, index) => {
      let sourcePath = frame;
      let line = 1;

      // Try to find the range for the function to resolve the actual source line
      for (const [sourceFilePath, sourceBpSet] of this.sourceFunctionBreakpoints.entries()) {
        if (sourceBpSet.has(frame) || sourceFilePath) {
          sourcePath = sourceFilePath;
          try {
            const { parseFunctionRanges } = require('./sourceBreakpoints');
            const ranges = parseFunctionRanges(sourcePath);
            const range = ranges.find((r: any) => r.name === frame);
            if (range) {
              line = range.startLine;
            }
          } catch (e) {
            // Ignore if parseFunctionRanges fails
          }
          break; // Stop looking after the first match
        }
      }

      return {
        id: index + 1,
        name: frame,
        source: sourcePath,
        line: line,
        column: 1
      };
    });
    this.state.args = inspection.args;
    this.state.variables = this.storageToVariables(storage);
  }

  private argsToVariables(args: string | undefined): Variable[] {
    if (!args) {
      return [{
        name: '(args)',
        value: '(none)',
        type: 'string',
        variablesReference: 0
      }];
    }

    try {
      const parsed = JSON.parse(args);
      return this.valueToVariables(parsed, 'arg');
    } catch {
      return [{
        name: '(args)',
        value: args,
        type: 'string',
        variablesReference: 0
      }];
    }
  }

  private valueToVariables(value: any, keyPrefix: string): Variable[] {
    if (Array.isArray(value)) {
      return value.slice(0, 100).map((item, index) => this.makeVariable(`${keyPrefix}${index}`, item));
    }

    if (value && typeof value === 'object') {
      return Object.keys(value)
        .sort((a, b) => a.localeCompare(b))
        .slice(0, 100)
        .map((key) => this.makeVariable(key, value[key]));
    }

    return [this.makeVariable(keyPrefix, value)];
  }

  private makeVariable(name: string, value: any): Variable {
    if (value === null || value === undefined) {
      return { name, value: String(value), type: 'null', variablesReference: 0 };
    }

    if (typeof value === 'string') {
      return { name, value, type: 'string', variablesReference: 0 };
    }

    if (typeof value === 'number' || typeof value === 'boolean') {
      return { name, value: String(value), type: typeof value, variablesReference: 0 };
    }

    if (Array.isArray(value)) {
      const ref = this.nextVarHandle++;
      this.variableHandles.set(ref, this.valueToVariables(value, `${name}[`));
      return { name, value: `Array(${value.length})`, type: 'array', variablesReference: ref };
    }

    if (typeof value === 'object') {
      const keys = Object.keys(value);
      const ref = this.nextVarHandle++;
      this.variableHandles.set(ref, this.valueToVariables(value, name));
      return { name, value: `Object(${keys.length})`, type: 'object', variablesReference: ref };
    }

    return { name, value: String(value), type: typeof value, variablesReference: 0 };
  }

  private storageToVariables(storage: Record<string, unknown>): Variable[] {
    return Object.entries(storage)
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([name, value]) => ({
        name,
        value: typeof value === 'string' ? value : JSON.stringify(value),
        type: Array.isArray(value) ? 'array' : typeof value,
        variablesReference: 0
      }));
  }

  public async stop(): Promise<void> {
    for (const reader of this.outputReaders) {
      reader.close();
    }
    this.outputReaders = [];

    if (this.debuggerProcess) {
      await this.debuggerProcess.stop();
      this.debuggerProcess = null;
    }

    this.state.isRunning = false;
    this.state.isPaused = false;
    this.state.callStack = [];
    this.state.variables = [];
    this.state.args = undefined;
    this.hasExecuted = false;
    this.sourceFunctionBreakpoints.clear();
  }

  private describeCapabilityFallback(bp: DebugProtocol.SourceBreakpoint): string | undefined {
    const notices: string[] = [];

    if (bp.condition && !this.backendCapabilities.conditionalBreakpoints) {
      notices.push('Conditional evaluation is unavailable in the current backend.');
    }
    if (bp.hitCondition && !this.backendCapabilities.hitConditionalBreakpoints) {
      notices.push('Hit-count filtering is unavailable in the current backend.');
    }
    if (bp.logMessage && !this.backendCapabilities.logPoints) {
      notices.push('Logpoints are unavailable in the current backend.');
    }

    return notices.length > 0 ? notices.join(' ') : undefined;
  }
}
