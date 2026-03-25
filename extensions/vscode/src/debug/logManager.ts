import * as vscode from 'vscode';
import * as fs from 'fs';
import * as path from 'path';

export enum LogLevel {
  Debug = 'DEBUG',
  Info = 'INFO',
  Warn = 'WARN',
  Error = 'ERROR'
}

export enum LogPhase {
  Lifecycle = 'LIFECYCLE',
  Spawn = 'SPAWN',
  Connect = 'CONNECT',
  Auth = 'AUTH',
  Load = 'LOAD',
  DAP = 'DAP',
  Backend = 'BACKEND',
  Teardown = 'TEARDOWN'
}

export interface LogEntry {
  timestamp: string;
  level: LogLevel;
  phase: LogPhase;
  message: string;
  correlationId?: string;
}

export class LogManager {
  private outputChannel: vscode.OutputChannel;
  private logFile: string;
  private maxLogSizeBytes = 10 * 1024 * 1024; // 10MB

  constructor(context: vscode.ExtensionContext) {
    if (typeof vscode !== 'undefined' && vscode.window) {
      this.outputChannel = vscode.window.createOutputChannel('Soroban Debugger');
    } else {
      this.outputChannel = {
        appendLine: (msg: string) => console.log(msg),
        dispose: () => {}
      } as any;
    }
    this.logFile = path.join(context.globalStorageUri.fsPath, 'debug.log');
    this.ensureLogDirectory();
  }

  private ensureLogDirectory(): void {
    const dir = path.dirname(this.logFile);
    if (!fs.existsSync(dir)) {
      fs.mkdirSync(dir, { recursive: true });
    }
  }

  public log(level: LogLevel, phase: LogPhase, message: string, correlationId?: string): void {
    const entry: LogEntry = {
      timestamp: new Date().toISOString(),
      level,
      phase,
      message: this.redact(message),
      correlationId
    };

    const formatted = this.formatEntry(entry);
    this.outputChannel.appendLine(formatted);
    this.persistEntry(formatted);
  }

  private formatEntry(entry: LogEntry): string {
    const cid = entry.correlationId ? ` [CID:${entry.correlationId}]` : '';
    return `[${entry.timestamp}] [${entry.level}] [${entry.phase}]${cid} ${entry.message}`;
  }

  private persistEntry(line: string): void {
    try {
      this.rotateLogIfNecessary();
      fs.appendFileSync(this.logFile, line + '\n', 'utf8');
    } catch (err) {
      // Fallback if file logging fails
      console.error('Failed to write to log file:', err);
    }
  }

  private rotateLogIfNecessary(): void {
    try {
      if (fs.existsSync(this.logFile) && fs.statSync(this.logFile).size > this.maxLogSizeBytes) {
        const backup = `${this.logFile}.old`;
        if (fs.existsSync(backup)) {
          fs.unlinkSync(backup);
        }
        fs.renameSync(this.logFile, backup);
      }
    } catch (err) {
      console.error('Log rotation failed:', err);
    }
  }

  private redact(message: string): string {
    // Redact --token <token>
    return message.replace(/(--token\s+)(\S+)/g, '$1[REDACTED]');
  }

  public dispose(): void {
    this.outputChannel.dispose();
  }
}
