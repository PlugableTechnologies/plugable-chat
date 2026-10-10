import { OPERATION_LABELS, formatBytes, type OperationEntry, type OperationKey, type OperationsMap } from './operations.ts';

export interface WelcomeInfo {
    modelName: string | null;
    modelSizeBytes: number | null;
    operations: OperationsMap;
}

function stageStatus(entry: OperationEntry | undefined, idleText: string): string {
    if (!entry) return idleText;
    if (entry.state === 'error') return `failed: ${entry.message} (use Retry in the status bar)`;
    if (entry.state === 'done') return 'ready';
    return entry.percent !== undefined ? `in progress, ${entry.percent}%` : 'in progress';
}

/** First-run welcome text. Names the model and size when known and reports each stage's status at the time it is shown. */
export function buildWelcomeMessage(info: WelcomeInfo): string {
    const model = info.modelName ?? 'the default chat model';
    const size = info.modelSizeBytes ? `, ${formatBytes(info.modelSizeBytes)}` : '';
    const stages: [OperationKey, string, string][] = [
        ['ep-registration', 'about 1.5 GB, only when a compatible GPU is found', 'not started or not needed on this computer'],
        ['embedding', 'used for search and database features', 'waiting'],
        ['model-download', `${model}${size}`, 'waiting'],
    ];
    const lines = stages.map(([key, detail, idle], i) =>
        `${i + 1}. **${OPERATION_LABELS[key]}** (${detail}): ${stageStatus(info.operations[key], idle)}`);
    return `## Welcome to Plugable Chat! 👋

Plugable Chat sets itself up on first run. Nothing needs to be installed first, but the downloads can take several minutes.

### First-run setup

${lines.join('\n')}

Live progress for each stage appears at the top of the window. The model name in the header changes from "Downloading" to the model's name when chat is ready.

### If something fails

- A red row at the top of the window shows the exact error with a **Retry** button.
- Check that this computer is online and has about 6 GB of free disk space.
- **Settings → Models** lets you pick a different model.
- If it keeps failing, send us the text shown in the red row.`;
}
