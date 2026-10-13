import { useEffect, useState } from 'react';
import { formatSecondsAsTime } from '../utils';
import { StatusRow } from '../StatusRow';

interface SearchingIndicatorProps {
    startTime: number;
    stage: 'indexing' | 'searching';
}

/** Live status line for RAG retrieval (indexing or searching documents). */
export const SearchingIndicator = ({ startTime, stage }: SearchingIndicatorProps) => {
    const [elapsed, setElapsed] = useState(0);

    useEffect(() => {
        const interval = setInterval(() => {
            setElapsed(Math.floor((Date.now() - startTime) / 1000));
        }, 1000);
        return () => clearInterval(interval);
    }, [startTime]);

    const label = stage === 'indexing' ? 'Indexing documents' : 'Searching documents';

    return <StatusRow label={label} details={[elapsed >= 1 ? formatSecondsAsTime(elapsed) : null]} isLive />;
};
