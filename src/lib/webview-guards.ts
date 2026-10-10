export interface KeyLike {
    key: string;
    ctrlKey?: boolean;
    metaKey?: boolean;
    shiftKey?: boolean;
}

/** F5, Ctrl/Cmd+R and Ctrl/Cmd+Shift+R reload the webview and drop in-memory state. */
export function isReloadShortcut(e: KeyLike): boolean {
    if (e.key === 'F5') return true;
    return !!(e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'r';
}

/**
 * In production builds the native webview context menu (which includes Reload) and the reload
 * shortcuts are disabled; the app offers its own context menu on chat items. Text inputs keep
 * the native menu so copy/paste still works.
 */
export function installProductionWebviewGuards(): void {
    document.addEventListener('contextmenu', (e) => {
        const t = e.target as HTMLElement | null;
        if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
        e.preventDefault();
    });
    document.addEventListener('keydown', (e) => {
        if (isReloadShortcut(e)) e.preventDefault();
    });
}
