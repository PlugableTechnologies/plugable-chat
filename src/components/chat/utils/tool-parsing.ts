// Tool parsing utilities for ChatArea

/**
 * Parsed tool call information
 */
export interface ParsedToolCallInfo {
    server: string;
    tool: string;
    arguments: Record<string, unknown>;
    rawContent: string;
}

/**
 * Parse tool call JSON to extract name, server, and arguments
 */
export function parseToolCallJsonFromContent(jsonContent: string): ParsedToolCallInfo | null {
    // Protect against giant payloads blocking the UI; fall back to raw display.
    if (jsonContent.length > 50000) {
        return {
            server: 'unknown',
            tool: 'large-payload',
            arguments: {},
            rawContent: jsonContent,
        };
    }
    try {
        const parsed = JSON.parse(jsonContent.trim());

        // Extract tool name - could be "name" or "tool_name" (GPT-OSS legacy)
        const fullName = parsed.name || parsed.tool_name || 'unknown';

        // Check if the name contains server prefix (server___tool format)
        let server = 'unknown';
        let tool = fullName;

        if (fullName.includes('___')) {
            const parts = fullName.split('___');
            server = parts[0];
            tool = parts.slice(1).join('___');
        } else if (parsed.server) {
            server = parsed.server;
        }

        // Extract arguments - could be "arguments", "parameters" (Llama), or "tool_args" (GPT-OSS)
        const args = parsed.arguments || parsed.parameters || parsed.tool_args || {};

        return {
            server,
            tool,
            arguments: args,
            rawContent: jsonContent,
        };
    } catch {
        return null;
    }
}

// ============ SQL Result Parsing ============

/**
 * SQL Result structure from sql_select tool
 */
export interface SqlResult {
    success: boolean;
    columns: string[];
    rows: (string | number | boolean | null)[][];
    row_count: number;
    rows_affected: number | null;
    error: string | null;
    sql_executed: string;
}

/**
 * Parse and validate SQL result from tool call result string
 */
export function parseSqlQueryResult(resultStr: string): SqlResult | null {
    try {
        const parsed = JSON.parse(resultStr);
        // Check for required SQL result fields
        if (
            typeof parsed === 'object' &&
            parsed !== null &&
            'success' in parsed &&
            'columns' in parsed &&
            'rows' in parsed &&
            Array.isArray(parsed.columns) &&
            Array.isArray(parsed.rows)
        ) {
            return parsed as SqlResult;
        }
        return null;
    } catch {
        return null;
    }
}

/**
 * Format a cell value for display
 */
export function formatSqlCellValue(value: string | number | boolean | null): string {
    if (value === null) return '—';
    if (typeof value === 'boolean') return value ? 'true' : 'false';
    if (typeof value === 'number') {
        // For decimals, limit to 2 decimal places for cleaner display
        if (!Number.isInteger(value)) {
            return value.toFixed(2).replace(/\.?0+$/, '');
        }
        return String(value);
    }
    return String(value);
}

/**
 * Determine if a column contains primarily numeric data
 */
export function isSqlColumnNumeric(rows: (string | number | boolean | null)[][], colIndex: number): boolean {
    let numericCount = 0;
    let totalNonNull = 0;
    for (const row of rows) {
        const val = row[colIndex];
        if (val !== null) {
            totalNonNull++;
            if (typeof val === 'number') numericCount++;
        }
    }
    return totalNonNull > 0 && numericCount / totalNonNull > 0.5;
}
