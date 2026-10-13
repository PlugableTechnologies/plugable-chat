import { useEffect, useState } from 'react';
import { formatSecondsAsTime } from '../utils';
import { StatusRow } from '../StatusRow';

interface ThinkingIndicatorProps {
    startTime: number;
}

/** Live "Reasoning" status line with elapsed time while the model is thinking. */
export const ThinkingIndicator = ({ startTime }: ThinkingIndicatorProps) => {
    const [elapsed, setElapsed] = useState(0);

    useEffect(() => {
        const interval = setInterval(() => {
            setElapsed(Math.floor((Date.now() - startTime) / 1000));
        }, 1000);
        return () => clearInterval(interval);
    }, [startTime]);

    return <StatusRow label="Reasoning" details={[elapsed >= 1 ? formatSecondsAsTime(elapsed) : null]} isLive />;
};
