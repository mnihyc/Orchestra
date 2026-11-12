/** Explicit public context enters one current reader's existing Ask draft, never a send. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { applyAskDraftSeed, askSeed, clearAskDraftSeed, queueAskDraft } from '@/features/ask/seed';
import { askDraftKey } from '@/features/composer/askDrafts';
import { useComposerStore } from '@/features/composer/Draft';
import { helpAskReference, helpAskText } from '@/features/help/ask';

const opened = vi.hoisted(() => vi.fn());
vi.mock('@/features/workbench/openView', () => ({ revealView: opened }));
const before = client.session.getState();
beforeEach(() => { clearAskDraftSeed(); opened.mockClear(); client.session.setState({ status: 'signed_in', userId: 'seed-reader', readerVersion: 10 }); });
afterEach(() => { clearAskDraftSeed(); client.session.setState(before, true); });

it('appends once to the actual draft without replacing its text or attachments', () => {
  const key = askDraftKey('root-a', null);
  const parts = [{ type: 'file' as const, blob: 'a'.repeat(64), path: '.orch/attachments/existing.txt', name: 'existing.txt', size: 4 }];
  useComposerStore.setState((current) => ({ text: { ...current.text, [key]: 'My current question' }, parts: { ...current.parts, [key]: parts } }));
  queueAskDraft('Public reference\n> Written summary');
  const id = askSeed.getState().seed?.id;
  expect(id).toBeDefined();
  expect(useComposerStore.getState().text[key]).toBe('My current question');
  expect(applyAskDraftSeed(id ?? '', key)).toBe(true);
  expect(useComposerStore.getState().text[key]).toBe('My current question\n\nPublic reference\n> Written summary');
  expect(useComposerStore.getState().parts[key]).toEqual(parts);
  expect(applyAskDraftSeed(id ?? '', key)).toBe(false);
  expect(opened).not.toHaveBeenCalled();
});

it('does not transfer an earlier reader opening to a new reader or draft', () => {
  queueAskDraft('Earlier reader selection');
  const id = askSeed.getState().seed?.id ?? '';
  client.session.setState({ userId: 'another-reader', readerVersion: 11 });
  const key = askDraftKey('root-a', null);
  expect(applyAskDraftSeed(id, key)).toBe(false);
  expect(useComposerStore.getState().text[key]).toBeUndefined();
  expect(askSeed.getState().seed).toBeNull();
});

it('keeps the latest explicit selection while an older effect catches up', () => {
  queueAskDraft('First public selection');
  const old = askSeed.getState().seed?.id ?? '';
  queueAskDraft('Second public selection');
  const latest = askSeed.getState().seed?.id ?? '';
  const key = askDraftKey('root-b', null);
  expect(applyAskDraftSeed(old, key)).toBe(false);
  expect(askSeed.getState().seed?.id).toBe(latest);
  expect(applyAskDraftSeed(latest, key)).toBe(true);
  expect(useComposerStore.getState().text[key]).toBe('Second public selection');
});

it('cancels unconsumed context on an empty replacement without changing an existing draft', () => {
  queueAskDraft('A pending public selection');
  queueAskDraft('  ');
  expect(askSeed.getState().seed).toBeNull();
});

it.each([
  ['Ask about this', 'Moving documents', 'Keep the original directory', 'moving'],
  ['就此提问', '移动文档', '保留原来的目录', 'moving'],
])('quotes only supplied public summaries and preserves exact article/heading references (%s)', (label, title, summary, article) => {
  const first = { article, heading: 'a section', title, summary: `${summary}\nSecond line` };
  const entries = [first, { article: 'empty-summary', title: 'A reference without a summary' }];
  expect(helpAskReference(first)).toBe('/help/moving#a%20section');
  expect(helpAskText(label, entries)).toBe(`${label}\n\n${title}\n/help/moving#a%20section\n> ${summary}\n> Second line\n\nA reference without a summary\n/help/empty-summary`);
});


it('retains a private seed until the actual normal draft has the same root', () => {
  queueAskDraft('Private source from root B', 'root-b');
  const pending = askSeed.getState().seed;
  if (pending === null) throw new Error('The explicit seed is present');
  const wrong = askDraftKey('root-a', null);
  const right = askDraftKey('root-b', null);
  useComposerStore.setState((before) => ({ text: { ...before.text, [wrong]: 'Existing A question', [right]: 'Existing B question' } }));
  expect(applyAskDraftSeed(pending.id, wrong, 'root-a')).toBe(false);
  expect(askSeed.getState().seed?.id).toBe(pending.id);
  expect(useComposerStore.getState().text[wrong]).toBe('Existing A question');
  expect(applyAskDraftSeed(pending.id, right, 'root-b')).toBe(true);
  expect(useComposerStore.getState().text[right]).toBe('Existing B question\n\nPrivate source from root B');
  expect(applyAskDraftSeed(pending.id, right, 'root-b')).toBe(false);
});

it('does not infer a private seed root from an arbitrary draft key', () => {
  queueAskDraft('Private root context', 'root-b');
  const pending = askSeed.getState().seed;
  if (pending === null) throw new Error('The explicit seed is present');
  expect(applyAskDraftSeed(pending.id, askDraftKey('root-b', null))).toBe(false);
  expect(askSeed.getState().seed?.id).toBe(pending.id);
});
