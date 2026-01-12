import { useEffect, useState } from 'react';
import { formatSecondsAsTime } from '../utils';

interface SearchingIndicatorProps {
    startTime: number;
    stage: 'indexing' | 'searching';
}

/**
 * Searching indicator component for RAG retrieval
 * Shows animated dots with indexing or searching label
 */
export const SearchingIndicator = ({ startTime, stage }: SearchingIndicatorProps) => {
    const [elapsed, setElapsed] = useState(0);

    useEffect(() => {
        const interval = setInterval(() => {
            setElapsed(Math.floor((Date.now() - startTime) / 1000));
        }, 1000);
        return () => clearInterval(interval);
    }, [startTime]);

    const label = stage === 'indexing' ? 'Indexing documents' : 'Searching documents';
    const color = stage === 'indexing' ? 'bg-blue-400' : 'bg-emerald-400';

    return (
        <div className="flex items-center gap-2 text-xs text-gray-500 mt-2 mb-1">
            <div className="flex gap-1">
                <div className={`w-1.5 h-1.5 ${color} rounded-full animate-pulse`} />
                <div className={`w-1.5 h-1.5 ${color} rounded-full animate-pulse`} style={{ animationDelay: '300ms' }} />
                <div className={`w-1.5 h-1.5 ${color} rounded-full animate-pulse`} style={{ animationDelay: '600ms' }} />
            </div>
            <span className="font-medium text-gray-500">
                {label}{elapsed >= 1 ? ` · ${formatSecondsAsTime(elapsed)}` : '...'}
            </span>
        </div>
    );
};
