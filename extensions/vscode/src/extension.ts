import * as vscode from 'vscode';
import {
  DebuggerProcessConfig,
  LaunchPreflightIssue,
  LaunchPreflightQuickFix,
  validateLaunchConfig
} from './cli/debuggerProcess';
import { SorobanDebugAdapterDescriptorFactory } from './debug/adapter';

type SorobanLaunchConfig = vscode.DebugConfiguration & DebuggerProcessConfig;

class SorobanDebugConfigurationProvider implements vscode.DebugConfigurationProvider {
  async resolveDebugConfiguration(
    folder: vscode.WorkspaceFolder | undefined,
    config: SorobanLaunchConfig
  ): Promise<vscode.DebugConfiguration | null | undefined> {
    if (!config.type && !config.request && !config.name) {
      return this.createDefaultLaunchConfig(folder);
    }

    if (config.type !== 'soroban' || config.request !== 'launch') {
      return config;
    }

    const preflight = await validateLaunchConfig(config);
    if (preflight.ok) {
      return config;
    }

    const issue = preflight.issues[0];
    const selected = await this.showPreflightError(issue);
    if (selected) {
      await this.applyQuickFix(selected, folder);
    }

    return undefined;
  }

  private async showPreflightError(issue: LaunchPreflightIssue): Promise<LaunchPreflightQuickFix | undefined> {
    const actions = issue.quickFixes.map(toQuickPickLabel);
    const selected = await vscode.window.showErrorMessage(
      `${issue.message} Expected: ${issue.expected}`,
      ...actions
    );
    return fromQuickPickLabel(selected);
  }

  private async applyQuickFix(
    quickFix: LaunchPreflightQuickFix,
    folder: vscode.WorkspaceFolder | undefined
  ): Promise<void> {
    switch (quickFix) {
      case 'pickBinary':
        await this.pickFile('Select soroban-debug binary', ['exe', 'bin', '']);
        return;
      case 'pickContract':
        await this.pickFile('Select Soroban contract WASM', ['wasm']);
        return;
      case 'pickSnapshot':
        await this.pickFile('Select snapshot JSON', ['json']);
        return;
      case 'openLaunchConfig':
        await vscode.commands.executeCommand('workbench.action.debug.configure');
        return;
      case 'generateLaunchConfig':
        await ensureLaunchConfig(folder);
        return;
      case 'openSettings':
        await vscode.commands.executeCommand('workbench.action.openSettings', '@ext:soroban.soroban-debugger');
        return;
      default:
        return;
    }
  }

  private async pickFile(title: string, extensions: string[]): Promise<void> {
    const filters = extensions.filter((ext) => ext.length > 0);
    const selected = await vscode.window.showOpenDialog({
      canSelectFiles: true,
      canSelectFolders: false,
      canSelectMany: false,
      openLabel: title,
      filters: filters.length > 0 ? { Files: filters } : undefined
    });

    if (selected && selected.length > 0) {
      await vscode.env.clipboard.writeText(selected[0].fsPath);
      await vscode.window.showInformationMessage(
        `Selected path copied to clipboard: ${selected[0].fsPath}`,
        'Open launch.json'
      ).then(async (choice) => {
        if (choice === 'Open launch.json') {
          await vscode.commands.executeCommand('workbench.action.debug.configure');
        }
      });
    }
  }

  private createDefaultLaunchConfig(folder: vscode.WorkspaceFolder | undefined): vscode.DebugConfiguration {
    const workspaceFolder = folder?.uri.fsPath ?? '${workspaceFolder}';
    return {
      name: 'Soroban: Debug Contract',
      type: 'soroban',
      request: 'launch',
      contractPath: `${workspaceFolder}/target/wasm32-unknown-unknown/release/contract.wasm`,
      snapshotPath: `${workspaceFolder}/snapshot.json`,
      entrypoint: 'main',
      args: [],
      trace: false,
      binaryPath: `${workspaceFolder}/target/debug/${process.platform === 'win32' ? 'soroban-debug.exe' : 'soroban-debug'}`
    };
  }
}

export function activate(context: vscode.ExtensionContext): void {
  const factory = new SorobanDebugAdapterDescriptorFactory(context);
  const configurationProvider = new SorobanDebugConfigurationProvider();

  const configProvider: vscode.DebugConfigurationProvider = {
    resolveDebugConfiguration(folder, config) {
      const settings = vscode.workspace.getConfiguration('soroban-debugger', folder);
      config.requestTimeoutMs = config.requestTimeoutMs ?? settings.get<number>('requestTimeoutMs');
      config.connectTimeoutMs = config.connectTimeoutMs ?? settings.get<number>('connectTimeoutMs');
      return config;
    }
  };

  context.subscriptions.push(
    vscode.debug.registerDebugAdapterDescriptorFactory('soroban', factory),
    vscode.debug.registerDebugConfigurationProvider('soroban', configurationProvider),
    factory
  );
}

export function deactivate(): void {
  // Cleanup on extension deactivation
}

async function ensureLaunchConfig(folder: vscode.WorkspaceFolder | undefined): Promise<void> {
  const workspaceFolder = folder ?? vscode.workspace.workspaceFolders?.[0];
  if (!workspaceFolder) {
    await vscode.window.showInformationMessage('Open a workspace folder first to generate a Soroban launch configuration.');
    return;
  }

  const vscodeDir = vscode.Uri.joinPath(workspaceFolder.uri, '.vscode');
  const launchUri = vscode.Uri.joinPath(vscodeDir, 'launch.json');

  try {
    await vscode.workspace.fs.createDirectory(vscodeDir);
    let launchJson: { version: string; configurations: vscode.DebugConfiguration[] };

    try {
      const existing = await vscode.workspace.fs.readFile(launchUri);
      launchJson = JSON.parse(Buffer.from(existing).toString('utf8')) as {
        version: string;
        configurations: vscode.DebugConfiguration[];
      };
    } catch {
      launchJson = { version: '0.2.0', configurations: [] };
    }

    const alreadyPresent = launchJson.configurations.some((configuration) =>
      configuration.type === 'soroban' &&
      configuration.request === 'launch'
    );

    if (!alreadyPresent) {
      launchJson.configurations.push({
        name: 'Soroban: Debug Contract',
        type: 'soroban',
        request: 'launch',
        contractPath: '${workspaceFolder}/target/wasm32-unknown-unknown/release/contract.wasm',
        snapshotPath: '${workspaceFolder}/snapshot.json',
        entrypoint: 'main',
        args: [],
        trace: false,
        binaryPath: '${workspaceFolder}/target/debug/soroban-debug'
      });

      await vscode.workspace.fs.writeFile(
        launchUri,
        Buffer.from(`${JSON.stringify(launchJson, null, 2)}\n`, 'utf8')
      );
    }

    const doc = await vscode.workspace.openTextDocument(launchUri);
    await vscode.window.showTextDocument(doc, { preview: false });
  } catch (error) {
    await vscode.window.showErrorMessage(`Failed to generate launch.json: ${String(error)}`);
  }
}

function toQuickPickLabel(quickFix: LaunchPreflightQuickFix): string {
  switch (quickFix) {
    case 'pickBinary':
      return 'Select Binary';
    case 'pickContract':
      return 'Select Contract';
    case 'pickSnapshot':
      return 'Select Snapshot';
    case 'openLaunchConfig':
      return 'Open launch.json';
    case 'generateLaunchConfig':
      return 'Generate Launch Config';
    case 'openSettings':
      return 'Open Settings';
    default:
      return quickFix;
  }
}

function fromQuickPickLabel(label: string | undefined): LaunchPreflightQuickFix | undefined {
  switch (label) {
    case 'Select Binary':
      return 'pickBinary';
    case 'Select Contract':
      return 'pickContract';
    case 'Select Snapshot':
      return 'pickSnapshot';
    case 'Open launch.json':
      return 'openLaunchConfig';
    case 'Generate Launch Config':
      return 'generateLaunchConfig';
    case 'Open Settings':
      return 'openSettings';
    default:
      return undefined;
  }
}
