import assert from 'node:assert/strict';
import { test } from 'node:test';
import { explainEmptyTables } from './empty-tables.ts';

// rc18 (Bob): "No database tables available" with no hint which of several causes it was.
test('no sources, toolbox off, nothing switched on and an unread cache each get their own explanation', () => {
    assert.match(explainEmptyTables(null).reason, /Settings > Databases/);
    assert.equal(explainEmptyTables({ enabled: true, sources: [] }).canRefresh, false);
    assert.match(explainEmptyTables({ enabled: true, sources: [] }).reason, /No database sources/);

    const off = explainEmptyTables({ enabled: false, sources: [{ enabled: true }] });
    assert.match(off.reason, /Database Toolbox is off/);
    assert.equal(off.canRefresh, false);

    const none = explainEmptyTables({ enabled: true, sources: [{ enabled: false }] });
    assert.match(none.reason, /No database source is switched on/);
    assert.equal(none.canRefresh, false);

    const unread = explainEmptyTables({ enabled: true, sources: [{ enabled: true }] });
    assert.match(unread.reason, /not been read yet/);
    assert.equal(unread.canRefresh, true);
});
