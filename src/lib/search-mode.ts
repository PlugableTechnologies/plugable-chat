// Which kind of search produced a result list. The backend falls back to keyword search when the
// embedding model is not ready; the UI must say so instead of passing it off as relevance ranking.

export const KEYWORD_ONLY_LABEL = 'Keyword search only (search model unavailable)';

export interface SearchModePayload {
    mode?: string | null;
    label?: string | null;
}

/** Text to show next to search results, or null when the search was the normal (semantic) one. */
export function searchModeLabel(payload: SearchModePayload | null | undefined): string | null {
    if (!payload || payload.mode !== 'keyword') return null;
    return payload.label && payload.label.trim() ? payload.label : KEYWORD_ONLY_LABEL;
}

/** Same for rows from `search_database_tables`, which carry `keyword_only` per row. */
export function tableSearchLabel(rows: ReadonlyArray<{ keyword_only?: boolean }> | null | undefined): string | null {
    return rows && rows.some((r) => r.keyword_only === true) ? KEYWORD_ONLY_LABEL : null;
}
