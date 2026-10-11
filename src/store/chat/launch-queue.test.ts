import assert from 'node:assert/strict';
import { test } from 'node:test';
import { takeNextLaunchPrompt } from './launch-queue.ts';

test('takes prompts from the front one at a time and leaves the rest', () => {
    const first = takeNextLaunchPrompt(['one', 'two', 'three']);
    assert.equal(first.nextLaunchPrompt, 'one');
    assert.deepEqual(first.remainingLaunchPrompts, ['two', 'three']);
    const second = takeNextLaunchPrompt(first.remainingLaunchPrompts);
    assert.equal(second.nextLaunchPrompt, 'two');
    assert.deepEqual(second.remainingLaunchPrompts, ['three']);
});

test('skips blank prompts and reports an empty queue as done', () => {
    assert.equal(takeNextLaunchPrompt(['  ', '', 'real']).nextLaunchPrompt, 'real');
    const done = takeNextLaunchPrompt([' ']);
    assert.equal(done.nextLaunchPrompt, null);
    assert.deepEqual(done.remainingLaunchPrompts, []);
    assert.equal(takeNextLaunchPrompt([]).nextLaunchPrompt, null);
});

test('does not change the queue it is given', () => {
    const queue = ['a', 'b'];
    takeNextLaunchPrompt(queue);
    assert.deepEqual(queue, ['a', 'b']);
});
