/** How a conversation is laid out and read: who shares a header, where the day changes and the new messages begin, which word is being mentioned, and the Markdown a message may hold, which is never raw HTML. */
import { describe, expect, it } from 'vitest';

import { type Block, parseBlocks, parseInline, placeOfAddress } from '@/features/chat/markup';
import { conversationRows, GROUP_WITHIN_MS, newCount } from '@/features/chat/rows';
import { editedParts, firstLine, insertMention, isBlank, mentionAt, textOfParts, textPart } from '@/features/chat/text';

import { ADA, at, GRACE, message } from '../doubles/chat-rows';

const dayOf = (moment: string) => moment.slice(0, 10);

describe('the rows of a conversation', () => {
  it('give consecutive messages of one author within five minutes one header', () => {
    const rows = conversationRows([message(1), message(2, { created_at: at(60) }), message(3, { created_at: at(600) })], { readTo: null, me: null, dayOf });
    const heads = rows.flatMap((row) => (row.kind === 'message' ? [row.head] : []));
    expect(heads).toEqual([true, false, true]);
    expect(GROUP_WITHIN_MS).toBe(300_000);
  });

  it('start a new header when the author changes', () => {
    const rows = conversationRows([message(1), message(2, { author: GRACE, created_at: at(5) })], { readTo: null, me: null, dayOf });
    expect(rows.flatMap((row) => (row.kind === 'message' ? [row.head] : []))).toEqual([true, true]);
  });

  it('put a day divider where the day changes and a header after it', () => {
    const next = message(2, { created_at: '2026-10-02T09:00:10.000Z' });
    const rows = conversationRows([message(1), next], { readTo: null, me: null, dayOf });
    expect(rows.map((row) => row.kind)).toEqual(['day', 'message', 'day', 'message']);
    expect(rows[3]).toMatchObject({ head: true });
  });

  it('mark where the new ones begin: above the first message after the position read that the reader did not write', () => {
    const rows = conversationRows([message(1), message(2, { author: GRACE }), message(3, { author: ADA }), message(4, { author: GRACE })], {
      readTo: 1,
      me: ADA.id,
      dayOf,
    });
    expect(rows.map((row) => (row.kind === 'message' ? row.message.seq_created : row.kind))).toEqual(['day', 1, 'new', 2, 3, 4]);
  });

  it('mark nothing when everything after the position was written by the reader, or when no position is known', () => {
    const mine = [message(1), message(2), message(3)];
    expect(conversationRows(mine, { readTo: 1, me: ADA.id, dayOf }).some((row) => row.kind === 'new')).toBe(false);
    expect(conversationRows([message(1, { author: GRACE })], { readTo: null, me: ADA.id, dayOf }).some((row) => row.kind === 'new')).toBe(false);
    expect(newCount([message(1, { author: GRACE }), message(2, { author: GRACE })], 1, ADA.id)).toBe(1);
  });

  it('make a group\'s event a line of its own that breaks a run of messages', () => {
    const event = message(2, {
      kind: 'system',
      author: null,
      parts: [],
      system: { event: 'member_added', by: ADA, user: GRACE, name: null },
    });
    const rows = conversationRows([message(1), event, message(3, { created_at: at(5) })], { readTo: null, me: null, dayOf });
    expect(rows.map((row) => row.kind)).toEqual(['day', 'message', 'system', 'message']);
    expect(rows[3]).toMatchObject({ head: true });
  });
});

describe('the word being mentioned', () => {
  it('is the name after an @ that begins a word, up to the caret', () => {
    expect(mentionAt('hello @gra', 10)).toEqual({ start: 6, end: 10, query: 'gra' });
    expect(mentionAt('@', 1)).toEqual({ start: 0, end: 1, query: '' });
    expect(mentionAt('thanks\n@do', 10)).toEqual({ start: 7, end: 10, query: 'do' });
  });

  it('is nothing in an address, or once the name has ended', () => {
    expect(mentionAt('write to ada@example', 20)).toBeNull();
    expect(mentionAt('hello @grace and', 16)).toBeNull();
  });

  it('is put in its place with a space after it, and the caret goes after the space', () => {
    expect(insertMention('hello @gra', { start: 6, end: 10, query: 'gra' }, 'grace')).toEqual({ text: 'hello @grace ', caret: 13 });
    expect(insertMention('hi @gr there', { start: 3, end: 6, query: 'gr' }, 'grace')).toEqual({ text: 'hi @grace there', caret: 10 });
  });
});

describe('the text of a message', () => {
  it('is no part at all when it says nothing', () => {
    expect(isBlank(' \n\t')).toBe(true);
    expect(textPart('  \n')).toEqual([]);
    expect(textPart(' hello ')).toEqual([{ type: 'text', text: 'hello' }]);
  });

  it('keeps what is not text when it is edited', () => {
    const quoted = { type: 'item_ref' as const, thread_id: 't', item_id: 'i' };
    const edited = editedParts([{ type: 'text', text: 'old' }, quoted], 'new');
    expect(edited).toEqual([{ type: 'text', text: 'new' }, quoted]);
    expect(textOfParts(edited)).toBe('new');
    expect(editedParts([quoted], '  ')).toEqual([quoted]);
  });

  it('has a first line that is the first one with something in it, cut to a length', () => {
    expect(firstLine([{ type: 'text', text: '\n\nfirst line here\nsecond' }], 10)).toBe('first line');
  });
});

const flat = (blocks: Block[]) => JSON.stringify(blocks);

describe('the Markdown of a message', () => {
  it('reads emphasis, code, strike and links', () => {
    const inline = parseInline('a **b** and *c* and `d` and ~~e~~ and [f](https://x.org/p) and https://y.org/q.');
    expect(inline.map((one) => one.type)).toEqual(['text', 'strong', 'text', 'em', 'text', 'code', 'text', 'del', 'text', 'link', 'text', 'link', 'text']);
    // The full stop after an address belongs to the sentence.
    expect(inline.at(-2)).toMatchObject({ type: 'link', href: 'https://y.org/q' });
  });

  it('reads a mention of a name or of all, and not an address or an @ in the middle of a word', () => {
    expect(parseInline('hi @grace and @all').filter((one) => one.type === 'mention')).toEqual([
      { type: 'mention', name: 'grace' },
      { type: 'mention', name: 'all' },
    ]);
    expect(parseInline('mail ada@example.org').some((one) => one.type === 'mention')).toBe(false);
  });

  it('reads paragraphs, a quotation, a list and a block of code', () => {
    const blocks = parseBlocks('one\ntwo\n\n> quoted\n\n- a\n- b\n\n1. x\n2. y\n\n```\ncode **not strong**\n```\n');
    expect(blocks.map((block) => block.type)).toEqual(['paragraph', 'quote', 'list', 'list', 'code']);
    expect(blocks[3]).toMatchObject({ type: 'list', ordered: true });
    expect(blocks[4]).toEqual({ type: 'code', text: 'code **not strong**' });
    expect(blocks[0]).toMatchObject({ children: [{ type: 'text', text: 'one' }, { type: 'break' }, { type: 'text', text: 'two' }] });
  });

  it('never makes an element of what is typed as one: markup stays text', () => {
    const blocks = parseBlocks('<script>alert(1)</script> <img src=x onerror=alert(1)> [x](javascript:alert(1)) <b>bold</b>');
    expect(flat(blocks)).not.toContain('"link"');
    const text = (blocks[0] as { children: { type: string; text?: string }[] }).children.map((one) => one.text ?? '').join('');
    expect(text).toContain('<script>alert(1)</script>');
    expect(text).toContain('<img src=x onerror=alert(1)>');
    expect(text).toContain('[x](javascript:alert(1))');
  });

  it('follows a link to http and https only', () => {
    expect(parseInline('[a](ftp://x.org/f)')[0]).toMatchObject({ type: 'text' });
    expect(parseInline('[a](http://x.org/f)')[0]).toMatchObject({ type: 'link', href: 'http://x.org/f' });
  });

  it('stops nesting deep emphasis instead of recursing without end', () => {
    const deep = '*'.repeat(200);
    expect(() => parseInline(deep)).not.toThrow();
    expect(() => parseBlocks(`${'> '.repeat(300)}x`)).not.toThrow();
  });

  it('knows an address that leads into this application and none that leads elsewhere', () => {
    expect(placeOfAddress('https://orch.test/topic/abc?tab=files', 'https://orch.test')).toEqual({ path: '/topic/abc?tab=files', id: 'abc', kind: 'topic' });
    expect(placeOfAddress('https://elsewhere.test/topic/abc', 'https://orch.test')).toBeNull();
    expect(placeOfAddress('https://orch.test/login', 'https://orch.test')).toBeNull();
  });
});
