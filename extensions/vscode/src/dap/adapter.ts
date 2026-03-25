import {
  DebugSession,
  InitializedEvent,
  StoppedEvent,
  ExitedEvent} from '@vscode/debugadapter';
import { DebugProtocol } from '@vscode/debugprotocol';
import * as readline from 'readline';
import { DebuggerProcess, DebuggerProcessConfig } from '../cli/debuggerProcess';
import { DebuggerState, Variable } from './protocol';
import { VariableStore } from './variableStore';
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
    storage: {}
  };
  private variableStore = new VariableStore();
  private threadId = 1;
  private outputReaders: readline.Interface[] = [];
  private hasExecuted = false;
  private exportedFunctions = new Set<string>();
  private sourceFunctionBreakpoints = new Map<string, Set<string>>();
  private functionBreakpointRefCounts = new Map<string, number>();
  private requestAbortControllers = new Map<number, AbortController>();
  private refreshAbortController: AbortController | null = null;
  private refreshGeneration = 0;

  protected initializeRequest(
    response: DebugProtocol.InitializeResponse,
    args: DebugProtocol.InitializeRequestArguments
  ): void {
    response.body = response.body || {};
    response.body.supportsConfigurationDoneRequest = true;
    response.body.supportsEvaluateForHovers = true;
    (response.body as any).supportsVariablePaging = true;
    (response.body as any).supportsCancelRequest = true;
    response.body.supportsSetVariable = false;
    response.body.supportsSetExpression = false;
    response.body.supportsConditionalBreakpoints = false;
    response.body.supportsHitConditionalBreakpoints = false;
    response.body.supportsLogPoints = false;

    this.sendResponse(response);
    this.sendEvent(new InitializedEvent());
  }

  protected async launchRequest(
    response: DebugProtocol.LaunchResponse,
    args: LaunchRequestArgs
  ): Promise<void> {
    try {
      this.debuggerProcess = new DebuggerProcess({
        contractPath: args.contractPath,
        snapshotPath: args.snapshotPath,
        entrypoint: args.entrypoint || 'main',
        args: args.args || [],
        trace: args.trace || false,
        binaryPath: args.binaryPath,
        port: args.port,
        token: args.token
      });

      await this.debuggerProcess.start();
      this.state.isRunning = true;
      this.state.isPaused = false;
      this.hasExecuted = false;
      this.variableStore.reset();
      this.exportedFunctions = await this.debuggerProcess.getContractFunctions();

      this.attachProcessListeners();
      this.sendResponse(response);
    } catch (error) {
      this.sendErrorResponse(response, {
        id: 1001,
        format: `Failed to launch debugger: ${error}`,
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

      await this.syncFunctionBreakpoints(
        source,
        new Set(
          resolved
            .filter((bp) => bp.verified && bp.functionName)
            .map((bp) => bp.functionName as string)
        )
      );

      this.state.breakpoints.set(source,
        breakpoints.map((bp) => ({
          source,
          line: bp.line,
          column: bp.column
        }))
      );

      response.body = {
        breakpoints: breakpoints.map((bp) => {
          const match = resolved.find((resolvedBreakpoint) => resolvedBreakpoint.line === bp.line);
          return {
            verified: match?.verified ?? false,
            line: bp.line,
            column: bp.column,
            source: args.source,
            message: match?.message
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
    this.variableStore.reset();

    const scopes: DebugProtocol.Scope[] = [];

    const argsRef = this.variableStore.createListHandle(this.variableStore.variablesFromArgs(this.state.args));
    scopes.push({
      name: 'Arguments',
      variablesReference: argsRef,
      expensive: false
    });

    const storageKeys = this.state.storage ? Object.keys(this.state.storage) : [];
    if (storageKeys.length > 0) {
      const variablesRef = this.variableStore.createListHandle(this.variableStore.variablesFromStorage(this.state.storage as Record<string, unknown>));

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
    const variables = this.variableStore.getVariables(args.variablesReference, {
      start: (args as any).start as number | undefined,
      count: (args as any).count as number | undefined
    });

    response.body = {
      variables: variables.map((v: Variable) => ({
        name: v.name,
        value: v.value,
        type: v.type,
        variablesReference: v.variablesReference || 0,
        indexedVariables: v.indexedVariables,
        namedVariables: v.namedVariables
      }))
    };

    this.sendResponse(response);
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
    if (this.debuggerProcess) {
      await this.refreshState();
      this.state.isPaused = true;
      this.sendEvent(new StoppedEvent('entry', this.threadId));
    }

    this.sendResponse(response);
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
      const requestSeq = (response as any).request_seq as number | undefined;
      const controller = new AbortController();
      if (typeof requestSeq === 'number') {
        this.requestAbortControllers.set(requestSeq, controller);
      }

      const result = await this.debuggerProcess.evaluate(args.expression, args.frameId, {
        signal: controller.signal
      });
      response.body = {
        result: result.result,
        type: result.type,
        variablesReference: result.variablesReference
      };
      this.sendResponse(response);
    } catch (error) {
      if ((error as any)?.name === 'AbortError' || (error as any)?.name === 'TimeoutError') {
        this.sendErrorResponse(response, {
          id: 1006,
          format: 'Evaluation canceled',
          showUser: false
        });
        return;
      }
      this.sendErrorResponse(response, {
        id: 1005,
        format: `${error}`,
        showUser: false
      });
    } finally {
      const requestSeq = (response as any).request_seq as number | undefined;
      if (typeof requestSeq === 'number') {
        this.requestAbortControllers.delete(requestSeq);
      }
    }
  }

  // VS Code will send a DAP "cancel" request with the requestId (seq) of the request to cancel.
  // We only support canceling long-running evaluate() calls at the moment.
  protected cancelRequest(response: any, args: any): void {
    const requestId = args?.requestId as number | undefined;
    if (typeof requestId === 'number') {
      const controller = this.requestAbortControllers.get(requestId);
      controller?.abort();
      this.requestAbortControllers.delete(requestId);
    }

    this.sendResponse(response);
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
      this.sendEvent(new LogOutputEvent(`${label} failed: ${error}\n`, LogLevel.Error));
    }
  }

  private async syncFunctionBreakpoints(source: string, nextFunctions: Set<string>): Promise<void> {
    if (!this.debuggerProcess) {
      return;
    }

    const previousFunctions = this.sourceFunctionBreakpoints.get(source) || new Set<string>();

    for (const functionName of previousFunctions) {
      if (nextFunctions.has(functionName)) {
        continue;
      }

      const count = (this.functionBreakpointRefCounts.get(functionName) || 1) - 1;
      if (count <= 0) {
        await this.debuggerProcess.clearBreakpoint(functionName);
        this.functionBreakpointRefCounts.delete(functionName);
      } else {
        this.functionBreakpointRefCounts.set(functionName, count);
      }
    }

    for (const functionName of nextFunctions) {
      if (previousFunctions.has(functionName)) {
        continue;
      }

      const count = this.functionBreakpointRefCounts.get(functionName) || 0;
      if (count === 0) {
        await this.debuggerProcess.setBreakpoint(functionName);
      }
      this.functionBreakpointRefCounts.set(functionName, count + 1);
    }

    this.sourceFunctionBreakpoints.set(source, nextFunctions);
  }

  private async refreshState(): Promise<void> {
    if (!this.debuggerProcess) {
      return;
    }

    this.refreshAbortController?.abort();
    const controller = new AbortController();
    this.refreshAbortController = controller;
    const generation = (this.refreshGeneration += 1);

    let inspection;
    let storage;
    try {
      [inspection, storage] = await Promise.all([
        this.debuggerProcess.inspect({ signal: controller.signal }),
        this.debuggerProcess.getStorage({ signal: controller.signal })
      ]);
    } catch (error) {
      if ((error as any)?.name === 'AbortError' || (error as any)?.name === 'TimeoutError') {
        return;
      }
      throw error;
    }

    if (controller.signal.aborted || generation !== this.refreshGeneration) {
      return;
    }

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
    this.state.storage = storage;
  }

  public async stop(): Promise<void> {
    this.refreshAbortController?.abort();
    this.refreshAbortController = null;

    for (const controller of this.requestAbortControllers.values()) {
      controller.abort();
    }
    this.requestAbortControllers.clear();

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
    this.state.storage = {};
    this.state.args = undefined;
    this.hasExecuted = false;
    this.sourceFunctionBreakpoints.clear();
    this.functionBreakpointRefCounts.clear();
  }
}
