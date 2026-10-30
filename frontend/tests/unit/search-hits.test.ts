// @vitest-environment node
/** What a hit of a search is to a reader: the match marked where the server counted it (in characters, not UTF-16 units), where it opens, the address that holds the page's state, and the days of a span. */
import { describe, expect, it } from 'vitest';

import { pathOf } from '@/features/finder/destinations';
import { dayOfMoment, momentOfDay } from '@/features/search/dates';
import { atOfHit, authorOfHit, crumbsOf, destinationOfHit, excerptOf, keyOfHit, wayOfHit } from '@/features/search/hits';
import { filtersOf, includes, kindsOf, narrowing, readParams, withKind } from '@/features/search/params';
import { tooShort } from '@/api/search';
import type { SearchHit } from '@/generated/api-types';

const NODE = (id: string, kind: 'org' | 'project' | 'topic' | 'thread', name: string) => ({ id, kind, name, redacted: false });

const itemHit = {
  kind: 'item',
  snippet: 'the redirect after login',
  matches: [[4, 12]],
  item: { id: 'item-1', created_at: '2026-10-01T09:00:00.000Z', author: { id: 'u1', kind: 'human', username: 'ada', display_name: 'Ada', avatar: { text: 'A', color: 0 } } },
  thread: NODE('thr', 'thread', 'Main thread'),
  topic: NODE('top', 'topic', 'Fix login'),
  project: NODE('prj', 'project', 'Web'),
} as unknown as SearchHit;

const chatHit = (node: ReturnType<typeof NODE> | null) =>
  ({
    kind: 'chat_message',
    snippet: 'redirect',
    matches: [[0, 8]],
    message: { id: 'msg-1', created_at: '2026-10-01T09:20:00.000Z', author: null },
    chat: { id: 'chat-1', kind: node === null ? 'direct' : 'node', name: null, node, org_id: 'org-1' },
    path: node === null ? [] : [NODE('o', 'org', 'Acme'), node],
  }) as unknown as SearchHit;

const documentHit = (block: string | null) =>
  ({
    kind: 'document',
    snippet: 'Release checklist',
    matches: [[0, 7]],
    block_id: block,
    document: { id: 'doc-1', title: 'Release checklist', updated_at: '2026-10-01T09:10:00.000Z', updated_by: null, created_by: { id: 'u2', kind: 'human', username: 'g', display_name: 'G', avatar: { text: 'G', color: 1 } } },
    path: [NODE('o', 'org', 'Acme'), NODE('prj', 'project', 'Web')],
  }) as unknown as SearchHit;

describe('the words around a match', () => {
  it('cuts the snippet along its matches', () => {
    expect(excerptOf('the redirect after login', [[4, 12]])).toEqual([
      { text: 'the ', marked: false },
      { text: 'redirect', marked: true },
      { text: ' after login', marked: false },
    ]);
  });

  it('counts the ranges in characters, as the server does, so that a letter after an emoji is marked right', () => {
    // The emoji is one character and two UTF-16 units: the match "ok" lies at characters 2 to 4.
    const parts = excerptOf('😀 ok fine', [[2, 4]]);
    expect(parts.filter((part) => part.marked).map((part) => part.text)).toEqual(['ok']);
    expect(parts.map((part) => part.text).join('')).toBe('😀 ok fine');
  });

  it('marks two matches that touch as one, ignores a range outside the snippet, and shows plain text for none', () => {
    expect(excerptOf('abcdef', [[0, 2], [1, 4], [90, 99]]).filter((part) => part.marked).map((part) => part.text)).toEqual(['abcd']);
    expect(excerptOf('abc', [])).toEqual([{ text: 'abc', marked: false }]);
  });
});

describe('where a hit opens and what it says', () => {
  it('opens the thread at the item, lit', () => {
    const destination = destinationOfHit(itemHit, () => null);
    expect(destination === null ? null : pathOf(destination)).toBe('/thread/thr?item=item-1');
  });

  it('opens a document at its block, or at its top when the title matched', () => {
    expect(pathOf(destinationOfHit(documentHit('blk-1'), () => null) ?? { to: '' })).toBe('/doc/doc-1?block=blk-1');
    expect(pathOf(destinationOfHit(documentHit(null), () => null) ?? { to: '' })).toBe('/doc/doc-1');
  });

  it('opens a place’s chat beside the place and a direct chat at its message in the org’s messages, if the org is known', () => {
    expect(pathOf(destinationOfHit(chatHit(NODE('top', 'topic', 'Fix login')), () => null) ?? { to: '' })).toBe('/topic/top?panel=chat&chat=chat-1&message=msg-1');
    expect(pathOf(destinationOfHit(chatHit(null), (org) => (org === 'org-1' ? 'acme' : null)) ?? { to: '' })).toBe('/org/acme/messages?chat=chat-1&message=msg-1');
    expect(destinationOfHit(chatHit(null), () => null)).toBeNull();
  });

  it('tells its crumbs: the places down to it, ending with the thing itself, and "Private project" for a place not to be named', () => {
    const words = { chat: 'Chat', private: (kind: string) => `Private ${kind}` };
    expect(crumbsOf(itemHit, words)).toEqual(['Web', 'Fix login', 'Main thread']);
    expect(crumbsOf(documentHit(null), words)).toEqual(['Acme', 'Web', 'Release checklist']);
    expect(crumbsOf(chatHit(NODE('top', 'topic', 'Fix login')), words)).toEqual(['Acme', 'Fix login']);
    expect(crumbsOf(chatHit(null), words)).toEqual(['Chat']);
    const hidden = { ...documentHit(null), path: [NODE('o', 'org', 'Acme'), { id: 'x', kind: 'project', name: null, redacted: true }] } as unknown as SearchHit;
    expect(crumbsOf(hidden, words)).toEqual(['Acme', 'Private project', 'Release checklist']);
  });

  it('tells where it lies, who wrote it, when, and a key of its own', () => {
    expect(wayOfHit(itemHit).map((place) => place.name)).toEqual(['Web', 'Fix login', 'Main thread']);
    expect(wayOfHit(documentHit(null)).map((place) => place.name)).toEqual(['Acme', 'Web']);
    expect(atOfHit(itemHit)).toBe('2026-10-01T09:00:00.000Z');
    expect(authorOfHit(itemHit)).toMatchObject({ display_name: 'Ada' });
    expect(authorOfHit(chatHit(null))).toBeNull();
    // One document found in two blocks is two results.
    expect(keyOfHit(documentHit('a'))).not.toBe(keyOfHit(documentHit('b')));
    expect(keyOfHit(documentHit(null))).not.toBe(keyOfHit(documentHit('a')));
  });
});

describe('the address of the search', () => {
  const id = '00000000-0000-4000-8000-000000000001';

  it('is read leniently: a value that is not what it should be is left out', () => {
    expect(readParams({ q: 'redirect', scope: id, kinds: 'agent_message,chat_message,nonsense', author: 'not-an-id', from: '2026-10-01', to: 'tomorrow' })).toEqual({
      q: 'redirect',
      scope: id,
      kinds: 'agent_message,chat_message',
      from: '2026-10-01',
    });
    expect(readParams({ q: '   ', kinds: 7 })).toEqual({});
  });

  it('turns into the filters of a request', () => {
    expect(filtersOf({ q: 'redirect', scope: id, kinds: 'document,user_message' })).toEqual({ q: 'redirect', scope: id, kinds: ['user_message', 'document'], author: undefined, from: undefined, to: undefined });
    expect(filtersOf({}).kinds).toBeUndefined();
  });

  it('keeps all kinds implicit and an empty selection explicit', () => {
    expect(includes({}, 'document')).toBe(true);
    expect(includes({ kinds: 'document' }, 'document')).toBe(true);
    expect(includes({ kinds: 'document' }, 'user_message')).toBe(false);
    expect(withKind({ kinds: 'document' }, 'user_message', true)).toBe('user_message,document');
    const fewer = withKind({}, 'chat_message', false);
    expect(fewer?.split(',')).not.toContain('chat_message');
    expect(withKind({ kinds: fewer }, 'chat_message', true)).toBeUndefined();
    expect(withKind({ kinds: 'document' }, 'document', false)).toBe('');
    expect(includes({ kinds: '' }, 'document')).toBe(false);
    expect(kindsOf('document,user_message')).toEqual(['user_message', 'document']);
  });

  it('counts the filters that narrow it', () => {
    expect(narrowing({ q: 'x' })).toBe(0);
    expect(narrowing({ q: 'x', scope: id, from: '2026-10-01' })).toBe(2);
    expect(narrowing({ scope: id, document_id: id, from: '2026-10-01', to: '2026-10-02' })).toBe(2);
  });
});

describe('words that are too short', () => {
  it('say what is short and why: nothing, one character, two without a place', () => {
    expect(tooShort('', { narrow: false })).toBe('none');
    expect(tooShort(' a ', { narrow: false })).toBe('short');
    expect(tooShort('ab', { narrow: false })).toBe('indexed');
    expect(tooShort('ab', { narrow: true })).toBeNull();
    expect(tooShort('abc', { narrow: false })).toBeNull();
  });

  it('counts characters and not UTF-16 units: two ideographs are two characters', () => {
    expect(tooShort('红包', { narrow: true })).toBeNull();
    expect(tooShort('😀', { narrow: true })).toBe('short');
  });
});

describe('the days of a span', () => {
  it('begin and end where the reader is, not in UTC', () => {
    expect(momentOfDay('2026-10-01', 'UTC', 'start')).toBe('2026-10-01T00:00:00.000Z');
    expect(momentOfDay('2026-10-01', 'UTC', 'end')).toBe('2026-10-01T23:59:59.999Z');
    expect(momentOfDay('2026-10-01', 'Asia/Shanghai', 'start')).toBe('2026-09-30T16:00:00.000Z');
    expect(momentOfDay('2026-10-01', 'America/New_York', 'end')).toBe('2026-10-02T03:59:59.999Z');
  });

  it('are read back as the day a moment falls on in that zone', () => {
    expect(dayOfMoment('2026-09-30T16:00:00.000Z', 'Asia/Shanghai')).toBe('2026-10-01');
    expect(dayOfMoment('2026-09-30T16:00:00.000Z', 'UTC')).toBe('2026-09-30');
    expect(momentOfDay('soon', 'UTC', 'start')).toBeUndefined();
  });

  it('take the day in UTC for a zone the browser does not know', () => {
    expect(momentOfDay('2026-10-01', 'Not/AZone', 'start')).toBe('2026-10-01T00:00:00.000Z');
  });
});
