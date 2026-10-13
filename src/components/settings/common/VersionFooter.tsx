import { useEffect, useState } from 'react';
import { Copy, Check } from 'lucide-react';
import { invoke } from '../../../lib/api';
import { formatVersionLine, formatVersionReport, type VersionInfo } from '../../../lib/version-info';
import { deviceFromModelId } from '../../../lib/gpu-assessment';
import { useChatStore } from '../../../store/chat-store';

// Release label and git commit, shown together because the numeric app version is the same for every
// release candidate. The copy button puts both in a bug report.
export function VersionFooter() {
    const [info, setInfo] = useState<VersionInfo | null>(null);
    const [copied, setCopied] = useState(false);
    const currentModel = useChatStore((state) => state.currentModel);
    const device = deviceFromModelId(currentModel);

    useEffect(() => {
        invoke<VersionInfo>('get_version_info').then(setInfo).catch((e) => console.error('get_version_info failed', e));
    }, []);

    const copy = async () => {
        try {
            await navigator.clipboard.writeText(formatVersionReport(info));
            setCopied(true);
            setTimeout(() => setCopied(false), 2000);
        } catch (e) {
            console.error('copy version failed', e);
        }
    };

    return (
        <div className="settings-version flex items-center justify-between px-6 py-2 border-t border-gray-100 text-xs text-gray-500">
            <span title={info?.gitSha}>
                Plugable Chat {formatVersionLine(info)}
                {device ? ` · running ${currentModel} on the ${device}` : ''}
            </span>
            <button onClick={copy} className="flex items-center gap-1 hover:text-gray-700" title="Copy version details">
                {copied ? <Check size={12} /> : <Copy size={12} />}
                {copied ? 'Copied' : 'Copy version'}
            </button>
        </div>
    );
}
