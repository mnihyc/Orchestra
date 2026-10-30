/** The real device controls retire exactly the records they successfully erase, including mounted writers. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Auth from '@/api/auth';
import type * as Personal from '@/api/personal';
import { client } from '@/api/client';
import { getCommentDrafts, forgetCommentDraftDeviceData, retireCommentDraftSession, setCommentText, commentSlotKey } from '@/features/documents/commentDrafts';
import { InterfaceSettings } from '@/features/settings/personal/Interface';
import { i18n } from '@/i18n';
import { me } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@/features/settings/PersonalFrame', () => ({ PersonalFrame: ({ children }: { children: ReactNode }) => <main>{children}</main> }));
vi.mock('@/features/settings/personal/Profile', () => ({ LookRows: () => null }));
vi.mock('@/api/personal', async (load) => ({ ...await load<typeof Personal>(), meStateQuery: { queryKey: ['device-personal'], queryFn: () => Promise.resolve({}) } }));
vi.mock('@/api/auth', async (load) => ({ ...await load<typeof Auth>(), meQuery: { queryKey: ['device-me'], queryFn: () => Promise.resolve(me(1, 'ada', 'Ada')) } }));
const session = client.session.getState(), person = 'device-comment-person';
const target = { kind: 'reply' as const, discussion: 'original-discussion' };
beforeEach(() => { localStorage.clear(); client.session.setState({ status: 'signed_in', userId: person, readerVersion: (session.readerVersion ?? 0) + 1 }); });
afterEach(() => { cleanup(); vi.restoreAllMocks(); forgetCommentDraftDeviceData(person, null); retireCommentDraftSession(); localStorage.clear(); client.session.setState(session); });

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
