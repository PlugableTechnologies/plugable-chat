/**
 * Why the "Attach Database" list can be empty, so the dialog names the cause instead of one generic
 * line. Only `canRefresh` causes get a Refresh button: reading tables cannot help while the toolbox
 * is off or no source is switched on.
 */
export interface ToolboxSummary {
    enabled: boolean;
    sources: Array<{ enabled: boolean }>;
}

export interface EmptyTablesExplanation {
    reason: string;
    canRefresh: boolean;
}

export function explainEmptyTables(config: ToolboxSummary | null | undefined): EmptyTablesExplanation {
    if (!config || config.sources.length === 0) {
        return { reason: 'No database sources are set up. Add one in Settings > Databases.', canRefresh: false };
    }
    if (!config.enabled) {
        return { reason: 'The Database Toolbox is off. Turn it on in Settings > Databases.', canRefresh: false };
    }
    if (!config.sources.some((s) => s.enabled)) {
        return { reason: 'No database source is switched on. Switch one on in Settings > Databases.', canRefresh: false };
    }
    return {
        reason: 'The tables of your databases have not been read yet. Refresh reads the table list so you can attach tables.',
        canRefresh: true,
    };
}
