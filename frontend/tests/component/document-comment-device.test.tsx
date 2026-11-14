/** The real device controls retire exactly the records they successfully erase, including mounted writers. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Auth from '@/api/auth';
import type * as Personal from '@/api/personal';
import { client } from '@/api/client';
import { meStateKey } from '@/api/personal';
import { getCommentDrafts, forgetCommentDraftDeviceData, retireCommentDraftSession, setCommentText, commentSlotKey } from '@/features/documents/commentDrafts';
import { InterfaceSettings } from '@/features/settings/personal/Interface';
import { i18n, setLocale } from '@/i18n';
import { forgetViewChoiceDeviceData, readViewChoice, useViewChoice, writeViewChoice } from '@/lib/viewChoices';
import { me } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@/features/settings/PersonalFrame', () => ({ PersonalFrame: ({ children }: { children: ReactNode }) => <main>{children}</main> }));
vi.mock('@/features/settings/personal/Profile', () => ({ LookRows: () => null }));
const writes = vi.hoisted(() => vi.fn<(key: string, value: unknown) => Promise<void>>());
vi.mock('@/api/personal', async (load) => ({ ...await load<typeof Personal>(), setMeState: writes, meStateQuery: { queryKey: ['device-personal'], queryFn: () => Promise.resolve({}) } }));
vi.mock('@/api/auth', async (load) => ({ ...await load<typeof Auth>(), meQuery: { queryKey: ['device-me'], queryFn: () => Promise.resolve(me(1, 'ada', 'Ada')) } }));
const session = client.session.getState(), person = 'device-comment-person';
const target = { kind: 'reply' as const, discussion: 'original-discussion' };
beforeEach(() => { writes.mockReset().mockResolvedValue(undefined); localStorage.clear(); client.session.setState({ status: 'signed_in', userId: person, readerVersion: (session.readerVersion ?? 0) + 1 }); });
afterEach(async () => { cleanup(); vi.restoreAllMocks(); forgetCommentDraftDeviceData(person, null); forgetViewChoiceDeviceData(person, null); retireCommentDraftSession(); localStorage.clear(); client.session.setState(session); await setLocale('en'); });

function OpenPageChoice() {
  const [choice] = useViewChoice('page', 'state', 'Default filters', 'screen-0');
  return <output aria-label="Mounted page filters">{choice}</output>;
}

it('saves reading choices under their own native key and preserves other interface and layout choices', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const named = { review: { title: 'Review' } };
  cache.setQueryData(meStateKey, { layouts: { named }, interface: { otherChoice: 'kept' } });
  const view = show(<InterfaceSettings />, cache);
  try {
    const recent = await screen.findByLabelText(i18n.t('settings.interface.reading.recent'));
    const delay = screen.getByLabelText(i18n.t('settings.interface.reading.delay'));
    await userEvent.clear(recent); await userEvent.type(recent, '27');
    await userEvent.clear(delay); await userEvent.type(delay, '333');
    const section = screen.getByRole('heading', { name: i18n.t('settings.interface.reading.title') }).closest('section');
    if (section === null) throw new Error('Reading choices need their own section.');
    await userEvent.click(within(section).getByRole('button', { name: i18n.t('common.save') }));
    await waitFor(() => expect(writes).toHaveBeenCalledWith('interface', { otherChoice: 'kept', kept_closed_tabs: 27, progress_delay_ms: 333 }));
    expect(cache.getQueryData(meStateKey)).toMatchObject({ layouts: { named }, interface: { otherChoice: 'kept', kept_closed_tabs: 27, progress_delay_ms: 333 } });
  } finally { view.unmount(); cache.clear(); }
});

it.each([['en', 'List filters and tabs'], ['zh-CN', '列表筛选和标签']] as const)('names one page-choice group and clears its real mounted values without touching another kind or person in %s', async (locale, label) => {
  await setLocale(locale);
  for (let at = 0; at < 40; at += 1) writeViewChoice(person, 'page', 'state', 'Saved filters', `screen-${at}`);
  writeViewChoice(person, 'resource-environments', 'listOrigin', 'Environment filters');
  writeViewChoice('another-person', 'page', 'state', 'Private filters', 'screen-0');
  show(<><OpenPageChoice /><InterfaceSettings /></>);
  const name = await screen.findByText(label);
  expect(screen.getAllByText(label)).toHaveLength(1);
  expect(screen.queryByText(i18n.t('craftChoiceNames.unknown'))).not.toBeInTheDocument();
  const row = name.closest('li'); if (row === null) throw new Error('The grouped choices need one ordinary device row');
  expect(within(row).getAllByRole('button')).toHaveLength(1);
  expect(row).not.toHaveTextContent(/screen-\d|Private filters/);
  expect(screen.getByLabelText('Mounted page filters')).toHaveTextContent('Saved filters');
  const environmentName = i18n.t('craftChoiceNames.scope', { kind: i18n.t('craftChoiceNames.kinds.resource-environments'), choice: i18n.t('craftChoiceNames.keys.listOrigin') });
  expect(screen.getByText(environmentName)).toBeVisible();
  await userEvent.click(within(row).getByRole('button'));
  expect(screen.getByLabelText('Mounted page filters')).toHaveTextContent('Default filters');
  for (let at = 0; at < 40; at += 1) expect(readViewChoice(person, 'page', 'state', 'Default filters', `screen-${at}`)).toBe('Default filters');
  expect(readViewChoice(person, 'resource-environments', 'listOrigin', '')).toBe('Environment filters');
  expect(readViewChoice('another-person', 'page', 'state', '', 'screen-0')).toBe('Private filters');
  expect(screen.queryByText(label)).not.toBeInTheDocument(); expect(screen.getByText(environmentName)).toBeVisible();
});

it('labels a mounted draft with human words and clearing it cannot be undone by that writer', async () => {
  const draft = getCommentDrafts(person, 'opaque-view', 'opaque-document'); setCommentText(draft, target, 'Forget these words');
  const user = userEvent.setup(); show(<InterfaceSettings />);
  const name = await screen.findByText(i18n.t('settings.interface.device.commentDrafts'));
  const row = name.closest('li'); if (row === null) throw new Error('The device setting must use its ordinary row');
  expect(row).not.toHaveTextContent('opaque-view'); expect(row).not.toHaveTextContent('opaque-document');
  await user.click(within(row).getByRole('button'));
  expect(localStorage.getItem(draft.key)).toBeNull(); expect(draft.store.getState().data.slots).toEqual({});
  expect(screen.queryByText(i18n.t('settings.interface.device.commentDrafts'))).not.toBeInTheDocument();
});

it('a partly refused Forget all clears successful writers and preserves the failed record and other person', async () => {
  const a = getCommentDrafts(person, 'view', 'a'), b = getCommentDrafts(person, 'view', 'b');
  setCommentText(a, target, 'First retained draft'); setCommentText(b, target, 'Second retained draft');
  const foreign = 'orch.another-person.unsaved.document-comments.private'; localStorage.setItem(foreign, 'Other person’s draft');
  const native = localStorage.removeItem.bind(localStorage);
  const failing = vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(function (this: Storage, key: string) {
    if (key === b.key) throw new DOMException('refused', 'SecurityError'); native(key);
  });
  const user = userEvent.setup(); show(<InterfaceSettings />);
  await user.click(await screen.findByRole('button', { name: i18n.t('settings.interface.device.forgetAll') }));
  const decision = screen.getByRole('alertdialog', { name: i18n.t('settings.interface.device.forgetAllTitle') });
  await user.click(within(decision).getByRole('button', { name: i18n.t('settings.interface.device.forgetAllConfirm') }));
  expect(localStorage.getItem(a.key)).toBeNull(); expect(a.store.getState().data.slots).toEqual({});
  expect(localStorage.getItem(b.key)).not.toBeNull();
  expect(b.store.getState().data.slots[commentSlotKey(target)]?.text).toBe('Second retained draft');
  expect(localStorage.getItem(foreign)).toBe('Other person’s draft'); expect(decision).toBeInTheDocument();
  failing.mockRestore();
  await user.click(within(decision).getByRole('button', { name: i18n.t('settings.interface.device.forgetAllConfirm') }));
  await waitFor(() => expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument());
  expect(b.store.getState().data.slots).toEqual({}); expect(localStorage.getItem(foreign)).toBe('Other person’s draft');
  act(() => forgetCommentDraftDeviceData(person, null));
});
