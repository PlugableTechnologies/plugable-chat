import { useMemo } from 'react';
import { type SqlResult, isSqlColumnNumeric, formatSqlCellValue } from '../utils';

interface SqlResultTableProps {
    sqlResult: SqlResult;
}

/**
 * SQL Result Table component - renders tabular data from sql_select results
 * Note: SQL executed details are shown in the tool call accordion, not duplicated here
 */
export const SqlResultTable = ({ sqlResult }: SqlResultTableProps) => {
    const { columns, rows, row_count } = sqlResult;

    // Pre-compute which columns are numeric for alignment
    const numericColumns = useMemo(() => {
        return columns.map((_, idx) => isSqlColumnNumeric(rows, idx));
    }, [columns, rows]);

    return (
        <div className="sql-result-table mt-2">
            {/* Data table */}
            <div className="overflow-x-auto rounded-lg border border-gray-200">
                <table className="min-w-full text-xs">
                    <thead>
                        <tr className="bg-gray-50 border-b border-gray-200">
                            {columns.map((col, idx) => (
                                <th
                                    key={idx}
                                    className={`px-3 py-2 font-semibold text-gray-700 ${numericColumns[idx] ? 'text-right' : 'text-left'
                                        }`}
                                >
                                    {col}
                                </th>
                            ))}
                        </tr>
                    </thead>
                    <tbody>
                        {rows.map((row, rowIdx) => (
                            <tr
                                key={rowIdx}
                                className={`border-b border-gray-100 ${rowIdx % 2 === 0 ? 'bg-white' : 'bg-gray-50/50'
                                    } hover:bg-blue-50/50 transition-colors`}
                            >
                                {row.map((cell, cellIdx) => (
                                    <td
                                        key={cellIdx}
                                        className={`px-3 py-2 ${numericColumns[cellIdx] ? 'text-right font-mono' : 'text-left'
                                            } ${cell === null ? 'text-gray-400 italic' : 'text-gray-800'}`}
                                    >
                                        {formatSqlCellValue(cell)}
                                    </td>
                                ))}
                            </tr>
                        ))}
                    </tbody>
                </table>
            </div>

            {/* Footer with row count */}
            <div className="mt-1 text-xs text-gray-500">
                {row_count === 1 ? '1 row' : `${row_count.toLocaleString()} rows`} returned
            </div>
        </div>
    );
};
