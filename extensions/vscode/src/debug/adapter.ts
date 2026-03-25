import * as vscode from 'vscode';
import { DebugAdapterDescriptor, DebugAdapterInlineImplementation } from 'vscode';
import { SorobanDebugSession } from '../dap/adapter';
import { LogManager } from './logManager';

export class SorobanDebugAdapterDescriptorFactory
  implements vscode.DebugAdapterDescriptorFactory, vscode.Disposable {

  private context: vscode.ExtensionContext;
  private logManager: LogManager;
  private session: SorobanDebugSession | null = null;

  constructor(context: vscode.ExtensionContext, logManager: LogManager) {
    this.context = context;
    this.logManager = logManager;
  }

  async createDebugAdapterDescriptor(
    session: vscode.DebugSession,
    executable: vscode.DebugAdapterExecutable | undefined
  ): Promise<DebugAdapterDescriptor | null> {
    this.session = new SorobanDebugSession(this.logManager);
    return new DebugAdapterInlineImplementation(this.session);
  }

  dispose(): void {
    this.session = null;
  }
}
