import type { AttachedTable } from '../../../store/chat-store';

interface AttachedTablePillsProps {
    tables: AttachedTable[];
    alwaysOnTables?: AttachedTable[];
    onRemove: (fqName: string) => void;
}

/**
 * Database Table Pills Component (supports both always-on/locked and removable pills)
 */
export const AttachedTablePills = ({
    tables,
    alwaysOnTables = [],
    onRemove
}: AttachedTablePillsProps) => {
    if (tables.length === 0 && alwaysOnTables.length === 0) return null;

    const truncateName = (name: string) => {
        if (name.length <= 20) return name;
        return name.slice(0, 17) + '...';
    };

    return (
        <div className="db-table-pill-bar flex flex-wrap gap-2 px-2 py-2 max-w-[900px] mx-auto">
            {/* Always-on tables - locked appearance, no remove button */}
            {alwaysOnTables.map((table) => (
                <div
                    key={`always-on-${table.tableFqName}`}
                    className="db-table-pill-locked inline-flex items-center gap-1.5 px-3 py-1.5 bg-amber-50 border border-amber-200 text-amber-700 rounded-full text-xs font-medium"
                    title={`${table.tableFqName} (always-on)`}
                >
                    <span>🔒</span>
                    <span>{truncateName(table.tableFqName)}</span>
                </div>
            ))}
            {/* Removable tables */}
            {tables.map((table) => (
                <div
                    key={table.tableFqName}
                    className="db-table-pill inline-flex items-center gap-1.5 px-3 py-1.5 bg-amber-100 text-amber-800 rounded-full text-xs font-medium group"
                    title={`${table.tableFqName} (${table.sourceName})`}
                >
                    <span>🗄️</span>
                    <span>{truncateName(table.tableFqName)}</span>
                    <button
                        onClick={() => onRemove(table.tableFqName)}
                        className="w-4 h-4 flex items-center justify-center rounded-full hover:bg-amber-200 text-amber-600 hover:text-amber-900 transition-colors"
                        title={`Remove ${table.tableFqName}`}
                    >
                        ×
                    </button>
                </div>
            ))}
        </div>
    );
};
