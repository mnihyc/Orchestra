/** People workflows preserve other work, current reading authority and deliberate unread positions. */
import { describe, expect, it } from 'vitest';
import { inverseOf } from '@/features/people/inverse';
import { candidates } from '@/features/people/MentionPicker';
import type { DocumentBlock, DocumentChanges, ChatState as RevisedChatState } from '@/generated/api-types';
import { mergeState } from '@/realtime/stores/chat';

const block = (id: string, text: string, more: Partial<DocumentBlock> = {}): DocumentBlock => ({ id, document_id: 'doc', parent_id: null, position: '1', type: 'paragraph', props: {}, content: [{ type: 'text', text }], removed: false, seq_created: 1, seq_updated: 1, updated_by: null, updated_at: '2026-10-01T00:00:00Z', ...more });
const changes = (rows: DocumentChanges['blocks']): DocumentChanges => ({ from: 1, to: null, authors: [], blocks: rows, title: null });
const read = (seq: number, revision: number): RevisedChatState => ({ chat_id: 'chat', level: null, effective_level: 'all', engaged: true, hidden: false, last_read_seq: seq, mentions_unread: 1, read_revision: revision });

describe('putting one shown block back', () => {
  it('keeps exact fractional sibling order and authored identifiers and external links', () => {
    const before = block('old', 'old', { position:'1.0000000000000000000000002', content:[{ type:'text',text:'old-child',href:'https://elsewhere.test/doc/doc?block=old-child' }] });
    const one = {id:before.id,before,after:null,by:null};
    const result = inverseOf(one, changes([one]), [block('earlier','kept',{position:'1.0000000000000000000000001'}),block('later','kept',{position:'1.0000000000000000000000003'})], () => 'fresh');
    expect(result.ops[0]).toMatchObject({op:'insert',after_id:'earlier',block:{content:[{text:'old-child',href:'https://elsewhere.test/doc/doc?block=old-child'}]}});
  });
  it('restores the selected text without making an operation for a colleague’s sibling', () => {
    const before = block('selected', 'before'); const after = block('selected', 'after', { seq_updated: 4 });
    const one = { id: after.id, before, after, by: null };
    const inverse = inverseOf(one, changes([one]), [after, block('sibling', 'colleague', { seq_updated: 8, position: '2' })], () => 'fresh');
    expect(inverse.ops).toEqual([{ op: 'update', id: 'selected', base_seq: 4, type: 'paragraph', props: {}, content: [{ type: 'text', text: 'before' }] }]);
  });
  it('refuses a newer selected after-state instead of overwriting it', () => {
    const before = block('selected', 'before'); const after = block('selected', 'after', { seq_updated: 4 });
    const one = { id: after.id, before, after, by: null };
    expect(inverseOf(one, changes([one]), [block('selected', 'new work', { seq_updated: 5 })], () => 'fresh').kind).toBe('stale');
  });
  it('does not remove an added subtree after somebody writes a child into it', () => {
    const after = block('added', 'new'); const one = { id: after.id, before: null, after, by: null };
    expect(inverseOf(one, changes([one]), [after, block('child', 'later', { parent_id: 'added' })], () => 'fresh').kind).toBe('stale');
  });
  it('returns a removed subtree under the live parent using fresh ids and fresh internal links', () => {
    const before = block('removed', 'gone', { parent_id: 'parent', position: '2' });
    const child = block('old-child', 'link', { parent_id: before.id, content: [{ type: 'text', text: 'jump', href: '/doc/doc?block=old-child' }] });
    const one = { id: before.id, before, after: null, by: null }; const childChange = { id: child.id, before: child, after: null, by: null };
    let id = 0;
    const inverse = inverseOf(one, changes([one, childChange]), [block('parent', 'live'), block('sibling', 'other', { parent_id: 'parent' })], () => `new-${++id}`);
    expect(inverse.kind).toBe('removed');
    expect(inverse.ops[0]).toMatchObject({ op: 'insert', parent_id: 'parent', after_id: 'sibling', block: { id: 'new-1', children: [{ id: 'new-2', content: [{ href: '/doc/doc?block=new-2' }] }] } });
  });
});

describe('deliberate unread across devices', () => {
  it('accepts a lower position only with a newer revision', () => expect(mergeState(read(12, 1), read(4, 2))).toEqual(read(4, 2)));
  it('an old automatic read cannot consume a new unread mark', () => expect(mergeState(read(4, 2), read(12, 1))).toEqual(read(4, 2)));
  it('normal reads on the same revision still advance', () => expect(mergeState(read(4, 2), read(8, 2)).last_read_seq).toBe(8));
});

it('the empty mention picker uses served current readers and actual broad counts', () => {
  const base = { user: null, available: true, reason: null, thread: null, agent: null, state: null, started_at: null };
  const choices = candidates([{ ...base, target: { kind: 'user', id: 'visible' }, name: 'vera', count: 1 }, { ...base, target: { kind: 'role', node_id: 'topic', role: 'viewer' }, name: 'Readers', count: 6, available: false, reason: 'Managers only' }], 0, '');
  expect(choices.map((one) => [one.name, one.count, one.available])).toEqual([['all', 0, true], ['vera', 1, true], ['Readers', 6, false]]);
});
