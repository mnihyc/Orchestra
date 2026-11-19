/** Who may take a thing away as a file, and what a menu says of it: offered to whoever holds the right, left out of a row's menu otherwise, and on the thing's own screen switched off with one line and the way to ask. */
import { describe, expect, it } from 'vitest';

import { exportEntries } from '@/features/export/menu';
import { exportOf, failureWords } from '@/features/export/jobs';
import { mayExportChat, mayExportThread } from '@/features/export/pending';
import { countOf } from '@/features/export/ThreadFile';
import type { Chat, Job, Thread } from '@/generated/api-types';
import { i18n } from '@/i18n';
import type { MenuEntry } from '@/ui/Menu';

const t = i18n.t.bind(i18n);

const ada = { display_name: 'Ada Lovelace' };
const nothing = (): void => undefined;
const idsOf = (entries: readonly MenuEntry[]): string[] => entries.flatMap((entry) => ('id' in entry ? [entry.id] : []));

const threadWith = (permissions: string[], can: Record<string, boolean> = {}): Thread =>
  ({ access: { permissions }, can: { archive: true, fork: true, place: false, ...can } }) as unknown as Thread;

const chatOf = (kind: Chat['kind'], members: string[] = [], can: Record<string, unknown> = {}): Chat =>
  ({ kind, members: members.map((id) => ({ id })), can: { send: true, moderate: false, reason: null, ...can } }) as unknown as Chat;

describe('whether a thread may be taken away as a file', () => {
  it('is what the thread says of the caller, whatever its page holds', () => {
    expect(mayExportThread(threadWith(['thread.view'], { export: true }))).toBe(true);
    expect(mayExportThread(threadWith(['thread.view', 'thread.export'], { export: false }))).toBe(false);
  });
});

describe('whether a chat may be taken away as a file', () => {
  it('is what the chat says of the caller, for a chat of a node and for a direct or a group chat alike', () => {
    expect(mayExportChat(chatOf('node', [], { export: true }))).toBe(true);
    expect(mayExportChat(chatOf('node', [], { export: false }))).toBe(false);
    expect(mayExportChat(chatOf('group', ['you'], { export: true }))).toBe(true);
    expect(mayExportChat(chatOf('direct', ['me', 'you'], { export: false }))).toBe(false);
  });
});

describe('Export… in a menu', () => {
  const entries = (allowed: boolean | null, where: 'row' | 'screen', reason?: string) =>
    exportEntries({ allowed, owner: ada, where, ...(reason === undefined ? {} : { reason }), onExport: nothing, onRequest: nothing, t });

  it('is offered, and does something, to whoever may take the thing away', () => {
    const [entry] = entries(true, 'screen');
    expect(entry && 'disabled' in entry && entry.disabled).toBeFalsy();
    expect(idsOf(entries(true, 'row'))).toEqual(['export']);
  });

  it('is left out of a row’s menu where the caller may not, and while nothing is known of what they hold', () => {
    expect(entries(false, 'row')).toEqual([]);
    expect(entries(null, 'screen')).toEqual([]);
  });

  it('stays on the thing’s own screen, switched off with one line that says for whom it is, with Request access under it', () => {
    const list = entries(false, 'screen');
    expect(idsOf(list)).toEqual(['export', 'export-request']);
    expect(list[0] && 'disabled' in list[0] && list[0].disabled).toBe('Taking this away as a file is for people Ada Lovelace names');
  });

  it('gives no way to ask where nobody could give the right', () => {
    const list = entries(false, 'screen', 'Only the people in this chat can take it away as a file');
    expect(idsOf(list)).toEqual(['export']);
  });
});

describe('a turn or a message number as typed', () => {
  it('is a whole number from 1, or nothing', () => {
    expect(countOf('')).toBeNull();
    expect(countOf(' 12 ')).toBe(12);
    expect(countOf('0')).toBe('invalid');
    expect(countOf('2.5')).toBe('invalid');
    expect(countOf('-3')).toBe('invalid');
  });
});

describe('the job of an archive', () => {
  const job = (result: unknown): Job => ({ result }) as unknown as Job;

  it('names the archive it made', () => {
    expect(exportOf(job({ export_id: 'abc' }))).toBe('abc');
    expect(exportOf(job(null))).toBeNull();
    expect(exportOf(job({ other: 1 }))).toBeNull();
  });

  it('says why it failed with the catalogued recovery and withholds unreviewed server diagnostics', () => {
    expect(failureWords({ code: 'export.too_large', message: 'raw', phase: 'write', undone: true }, t)).toBe(t('errors.export.too_large'));
    // Public errors now carry a translated next act, including known internal failures.
    expect(failureWords({ code: 'internal', message: 'the disk is full', phase: 'store', undone: true }, t)).toBe(t('errors.internal'));
    expect(failureWords(null, t)).toBe(t('export.job.failedAbout'));
  });
});
