/** Text search over the Models list: every word must match the name, alias, display name, device or task. */
export interface FilterableModel {
    name: string;
    alias?: string;
    displayName?: string;
    task?: string;
    runtime?: { deviceType?: string; executionProvider?: string };
}

export function matchesModelQuery(model: FilterableModel, query: string): boolean {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    if (words.length === 0) return true;
    const haystack = [
        model.name,
        model.alias,
        model.displayName,
        model.task,
        model.runtime?.deviceType,
        model.runtime?.executionProvider,
    ]
        .filter(Boolean)
        .join(' ')
        .toLowerCase();
    return words.every((w) => haystack.includes(w));
}
