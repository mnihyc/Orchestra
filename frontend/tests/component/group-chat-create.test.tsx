/** Explicit groups keep their home/visibility; one-person direct messages retain the native deduplication path. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client, ReceiptAcknowledged } from '@/api/client';
import type * as Chats from '@/api/chats';
import type * as Access from '@/api/access';
import type * as Recents from '@/api/pickerRecents';
import { NewChat, PeoplePicker } from '@/features/chat/NewChat';
import type { PlacePickerProps } from '@/features/settings/PlacePicker';
import { i18n, setLocale } from '@/i18n';
import { ADA, GRACE, DORA, chat } from '../doubles/chat-rows';
import { show } from './support';
const calls = vi.hoisted(() => ({ create: vi.fn<typeof Chats.createChat>(), opened: vi.fn<(chat: Awaited<ReturnType<typeof Chats.createChat>>) => void>(), close: vi.fn<(open: boolean) => void>() }));
vi.mock('@/api/chats', async (original) => ({ ...await original<typeof Chats>(), createChat: (...args: Parameters<typeof Chats.createChat>) => calls.create(...args) }));
const peopleSource = vi.hoisted(() => ({ read: vi.fn<(q: string, org: string | null, external: boolean, cursor: string | null) => Promise<import('@/generated/api-types').UserSearchReply>>(), record: vi.fn<typeof Recents.recordPickerChoice>(), recent: [] as string[] }));
vi.mock('@/api/access', async (load) => {
  const actual = await load<typeof Access>();
  return { ...actual, personPickerQuery: (q: string, org: string | null, external = false) => ({
    ...actual.personPickerQuery(q, org), queryKey: ['chat-picker-people', q, org, external],
    queryFn: ({ pageParam }: { pageParam: string | null }) => peopleSource.read(q, org, external, pageParam),
  }) };
});
vi.mock('@/api/pickerRecents', async (load) => {
  const actual = await load<typeof Recents>();
  return { ...actual, pickerRecentsQuery: (person: string) => ({ ...actual.pickerRecentsQuery(person), queryFn: () => Promise.resolve({ count: 5, choices: { user: peopleSource.recent } }) }), recordPickerChoice: peopleSource.record };
});
vi.mock('@/features/settings/PlacePicker', () => ({ PlacePicker: ({ onChange, rootOrgId, includeThreads, value }: PlacePickerProps) => <button type="button" onClick={() => onChange({ id: 'release-topic', kind: 'topic', color: null, folder: false, path: 'Studio / Release' })}>{`${rootOrgId}:${includeThreads === true ? 'threads' : ''}:${value?.path ?? ''}`}</button> }));
const before = client.session.getState();
const home = { id: 'studio', kind: 'org' as const, color: null, folder: false, path: 'Studio' };
const groupName = () => `${i18n.t('chat.new.name')} ${i18n.t('ui.field.optional')}`;
const draw = () => show(<NewChat open onOpenChange={calls.close} orgId="studio" me={ADA.id} defaultHome={home} onOpened={calls.opened} />);
beforeEach(() => { vi.clearAllMocks(); peopleSource.recent = []; peopleSource.record.mockResolvedValue({ count: 5, remembered: true });
  peopleSource.read.mockImplementation(() => Promise.resolve({ items: [GRACE, DORA], next_cursor: null, org_member_ids: [GRACE.id, DORA.id] })); client.session.setState({ status: 'signed_in', userId: ADA.id, readerVersion: 501 }); calls.create.mockResolvedValue(chat({ kind: 'group', node: null, members: [ADA, GRACE], name: 'Review' })); });
afterEach(async () => { cleanup(); client.session.setState(before); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('creates an explicit one-recipient group with the actual home and public visibility in %s', async (locale) => {
  await setLocale(locale); const user = userEvent.setup(); draw();
  await user.click(screen.getByRole('combobox', { name: i18n.t('chat.people.search') })); await user.click(await screen.findByRole('option', { name: /Grace Hopper/ }));
  await user.click(screen.getByRole('switch', { name: i18n.t('chat.new.asGroup') }));
  expect(screen.getAllByRole('textbox')[0]).toBe(screen.getByRole('textbox', { name: groupName() }));
  await user.type(screen.getByRole('textbox', { name: groupName() }), 'Review');
  await user.click(screen.getByRole('button', { name: 'studio:threads:Studio' }));
  await user.click(screen.getByRole('combobox', { name: i18n.t('chat.group.visibility') }));
  await user.click(screen.getByRole('option', { name: i18n.t('chat.group.public') }));
  expect(calls.create).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('chat.new.group', { count: 2 }) }));
  await waitFor(() => expect(calls.create).toHaveBeenCalledWith('studio', { user_ids: [GRACE.id], name: 'Review', home_id: 'release-topic', visibility: 'public' }, expect.any(String)));
  expect(calls.opened).toHaveBeenCalledOnce(); expect(calls.close).toHaveBeenCalledWith(false);
});

it('keeps the ordinary direct-chat body free of group fields', async () => {
  draw(); await userEvent.click(screen.getByRole('combobox', { name: i18n.t('chat.people.search') })); await userEvent.click(await screen.findByRole('option', { name: /Grace Hopper/ }));
  expect(screen.queryByRole('textbox', { name: groupName() })).toBeNull();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('chat.new.direct', { name: GRACE.display_name }) }));
  await waitFor(() => expect(calls.create).toHaveBeenCalledWith('studio', { user_ids: [GRACE.id] }, expect.any(String)));
});

it('keeps an automatic group explicit after a recipient is removed and retains a failed draft and key', async () => {
  calls.create.mockRejectedValueOnce(new Error('Lost response')).mockResolvedValueOnce(chat({ kind: 'group', node: null, members: [ADA, GRACE] }));
  draw(); const user = userEvent.setup();
  await user.click(screen.getByRole('combobox', { name: i18n.t('chat.people.search') })); await user.click(await screen.findByRole('option', { name: /Grace Hopper/ })); await user.click(screen.getByRole('combobox', { name: i18n.t('chat.people.search') })); await user.click(await screen.findByRole('option', { name: /Dora Developer/ }));
  await user.type(screen.getByRole('textbox', { name: groupName() }), 'Review');
  await user.click(screen.getByRole('button', { name: /Dora Developer/ }));
  expect(screen.getByRole('switch', { name: i18n.t('chat.new.asGroup') })).toBeChecked();
  await user.click(screen.getByRole('button', { name: i18n.t('chat.new.group', { count: 2 }) }));
  await screen.findByRole('alert');
  expect(screen.getByRole('textbox', { name: groupName() })).toHaveValue('Review');
  await user.click(screen.getByRole('button', { name: i18n.t('chat.new.group', { count: 2 }) }));
  await waitFor(() => expect(calls.create).toHaveBeenCalledTimes(2));
  expect(calls.create.mock.calls[0]?.[2]).toBe(calls.create.mock.calls[1]?.[2]);
});

it('acknowledges committed creation without manufacturing a readable Chat', async () => {
  calls.create.mockRejectedValue(new ReceiptAcknowledged({ projection: 'receipt', chat_id: 'made-group', created: true }, 201, 'create-group'));
  draw(); await userEvent.click(screen.getByRole('combobox', { name: i18n.t('chat.people.search') })); await userEvent.click(await screen.findByRole('option', { name: /Grace Hopper/ }));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('chat.new.direct', { name: GRACE.display_name }) }));
  await waitFor(() => expect(calls.close).toHaveBeenCalledWith(false));
  expect(calls.opened).not.toHaveBeenCalled(); expect(screen.queryByRole('alert')).toBeNull();
});

it('retires the old actor selection and ignores an old accepted response', async () => {
  let finish: ((row: Awaited<ReturnType<typeof Chats.createChat>>) => void) | undefined;
  calls.create.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  draw(); await userEvent.click(screen.getByRole('combobox', { name: i18n.t('chat.people.search') })); await userEvent.click(await screen.findByRole('option', { name: /Grace Hopper/ }));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('chat.new.direct', { name: GRACE.display_name }) }));
  const settle = finish; if (settle === undefined) throw new Error('Creation did not start');
  act(() => client.session.setState({ readerVersion: 502 }));
  await act(async () => { settle(chat({ kind: 'group' })); await Promise.resolve(); });
  expect(calls.opened).not.toHaveBeenCalled(); expect(calls.close).not.toHaveBeenCalled();
  expect(screen.getByRole('button', { name: i18n.t('chat.new.button') })).toHaveAttribute('aria-disabled', 'true');
});

it.each(['en', 'zh-CN'] as const)('starts with recent people and keeps actual membership and chosen people across native search pages in %s', async (locale) => {
  await setLocale(locale); peopleSource.recent = [GRACE.id];
  const outside = { ...DORA, id: 'guest', username: 'guest', display_name: 'Guest person' };
  peopleSource.read.mockImplementation((q: string, _org: string | null, _external: boolean, cursor: string | null) => Promise.resolve(
    q === '' ? { items: [DORA, GRACE], next_cursor: 'more', org_member_ids: [DORA.id, GRACE.id] }
      : cursor === null ? { items: [GRACE], next_cursor: 'more', org_member_ids: [GRACE.id] }
        : { items: [outside], next_cursor: null, org_member_ids: [] },
  ));
  const toggle = vi.fn(); const user = userEvent.setup();
  show(<PeoplePicker orgId="studio" chosen={[DORA]} onToggle={toggle} exclude={new Set()} guests me={ADA.id} />);
  const input = screen.getByRole('combobox', { name: i18n.t('chat.people.search') }); await user.click(input);
  expect(await screen.findByRole('option', { name: GRACE.display_name })).toBeVisible();
  expect(screen.getAllByRole('option')[0]).toHaveAccessibleName(GRACE.display_name);
  await user.type(input, 'g');
  await waitFor(() => expect(peopleSource.read).toHaveBeenCalledWith('g', 'studio', true, null));
  expect(await screen.findByRole('option', { name: GRACE.display_name })).not.toHaveTextContent(i18n.t('chat.people.guestChip'));
  await user.click(screen.getByRole('button', { name: i18n.t('ui.pagination.more') }));
  expect(await screen.findByRole('option', { name: outside.display_name })).toHaveTextContent(i18n.t('chat.people.guestChip'));
  expect(screen.getByRole('button', { name: DORA.display_name })).toBeVisible();
  await user.click(input); await user.keyboard('{End}{Enter}'); expect(toggle).toHaveBeenCalledExactlyOnceWith(outside);
  await waitFor(() => expect(peopleSource.record).toHaveBeenCalledWith({ kind: 'user', key: outside.id }));
});
