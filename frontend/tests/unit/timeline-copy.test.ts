/** What Copy gives of the items picked in a conversation: Markdown in their order, a command and its output fenced, a change as a diff, and a link that names them all. */
import { describe, expect, it } from 'vitest';

import { itemsLink } from '@/features/timeline/ItemActions';
import { itemMarkdown, selectionMarkdown } from '@/features/timeline/SelectionBar';

import { agentMessage, change, command, fileChange } from '../doubles/thread-rows';

describe('copying what is picked', () => {
  it('fences a command with its output, in a fence its output cannot close', () => {
    expect(itemMarkdown(command('c1', { command: 'ls', output: 'a\nb', exit: 0 }))).toBe('```sh\n$ ls\na\nb\n```');
    expect(itemMarkdown(command('c2', { command: 'echo', output: 'x ``` y', exit: 0 }))).toBe('````sh\n$ echo\nx ``` y\n````');
  });

  it('gives a change as a diff, and a change without one by its path', () => {
    const item = fileChange('f1', { changes: [change('a.ts', '-old\n+new'), change('b.bin', null)] });
    expect(itemMarkdown(item)).toBe('```diff\n-old\n+new\n```\n\n`b.bin`');
  });

  it('joins what was said and done in its order, leaving out what says nothing', () => {
    const items = [agentMessage('a1', 'First.'), command('c1', { command: 'pwd', output: '/work', exit: 0 }), agentMessage('a2', '')];
    expect(selectionMarkdown(items)).toBe('First.\n\n```sh\n$ pwd\n/work\n```');
  });

  it('links to the thread with the first item brought into view and all of them named', () => {
    const link = new URL(itemsLink('t1', ['i1', 'i2', 'i3']));
    expect(link.pathname).toBe('/thread/t1');
    expect(link.searchParams.get('item')).toBe('i1');
    expect(link.searchParams.get('items')).toBe('i1,i2,i3');
    expect(new URL(itemsLink('t1', ['i1'])).searchParams.get('items')).toBeNull();
  });
});
