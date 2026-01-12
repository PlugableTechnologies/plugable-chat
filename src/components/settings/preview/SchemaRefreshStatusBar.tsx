import { useState, useEffect } from 'react';
import { Loader2, CheckCircle, XCircle } from 'lucide-react';
import type { SchemaRefreshStatus } from '../types';

interface SchemaRefreshStatusBarProps {
    status: SchemaRefreshStatus;
}

export function SchemaRefreshStatusBar({ status }: SchemaRefreshStatusBarProps) {
    const [elapsed, setElapsed] = useState(0);

    useEffect(() => {
        if (status.is_complete) return;
        const interval = setInterval(() => {
            setElapsed(Math.floor((Date.now() - status.startTime) / 1000));
        }, 1000);
        return () => clearInterval(interval);
    }, [status.startTime, status.is_complete]);

    const formatElapsedTime = (seconds: number): string => {
        if (seconds < 60) return `${seconds}s`;
        const mins = Math.floor(seconds / 60);
        const secs = seconds % 60;
        return `${mins}m ${secs}s`;
    };

    return (
        <div className={`flex items-center justify-between px-4 py-2 border rounded-lg transition-colors ${status.error ? 'bg-red-50 border-red-200 text-red-800' : 'bg-blue-50 border-blue-200 text-blue-800'}`}>
            <div className="flex items-center gap-3 min-w-0">
                {!status.is_complete && !status.error && <Loader2 className="animate-spin text-blue-600" size={16} />}
                {status.is_complete && <CheckCircle className="text-green-600" size={16} />}
                {status.error && <XCircle className="text-red-600" size={16} />}
                <div className="flex flex-col min-w-0">
                    <span className="text-sm font-medium truncate">
                        {status.error ? `Error: ${status.error}` : status.message}
                    </span>
                    {status.current_table && !status.is_complete && (
                        <span className="text-[10px] opacity-70 truncate">{status.current_table}</span>
                    )}
                </div>
            </div>
            {!status.is_complete && !status.error && (
                <span className="font-mono text-xs opacity-70 ml-4 flex-shrink-0">
                    {formatElapsedTime(elapsed)}
                </span>
            )}
        </div>
    );
}
