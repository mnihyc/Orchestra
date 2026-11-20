/** Current message reads, native audience facts and both administrator composers share the same controls. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { focusManager } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Messages from '@/api/adminMessages';
import type * as Nodes from '@/api/nodes';
import type * as Settings from '@/api/settings';
import { ApiError } from '@/api/client';
import { MessageComposer, type MessageScope } from '@/features/admin-messages/Composer';
import { AdministratorMessage } from '@/features/admin-messages/Message';
import { MessagesSection } from '@/features/admin-messages/Sent';
import { TellMembers } from '@/features/admin-messages/TellMembers';
import { SettingRow } from '@/features/settings/SettingRow';
import { localizedSetting } from '@/features/settings/settingText';
import { withGroup } from '@/features/settings/settingsModel';
import type { AdminMessage, AdminMessageListResponse, AdminMessageSummary, Node, ResolvedSetting, SettingDef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { org } from '../doubles/fixtures';
import { ADA } from '../unit/general-access-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({
  send: vi.fn<typeof Messages.sendMessage>(),
  list: vi.fn<(org: string | null, cursor: string | null) => Promise<AdminMessageListResponse>>(),
  read: vi.fn<(id: string) => Promise<AdminMessage>>(),
  node: vi.fn<(id: string) => Promise<Node>>(),
  put: vi.fn<typeof Settings.putSetting>(),
  canManageMembers: true,
}));
vi.mock('@/api/adminMessages', () => ({
  sendMessage: calls.send,
  sentMessagesQuery: (org: string | null) => ({
    queryKey: ['admin-messages', org ?? 'system'], initialPageParam: null as string | null,
    queryFn: ({ pageParam }: { pageParam: string | null }) => calls.list(org, pageParam),
    getNextPageParam: (page: AdminMessageListResponse) => page.next_cursor,
  }),
  messageQuery: (id: string) => ({ queryKey: ['admin-message', id], queryFn: () => calls.read(id) }),
}));
vi.mock('@/api/nodes', async (load) => ({ ...await load<typeof Nodes>(),
  nodeQuery: (id: string) => ({ queryKey: ['message-decision-org', id], queryFn: () => calls.node(id) }),
}));
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof Settings>(), putSetting: calls.put }));

const ID = '00000000-0000-4000-8000-000000000321';
const OLDER = '00000000-0000-4000-8000-000000000322';
const orgRef = { id: 'acme', kind: 'org', name: 'Acme', redacted: false } as const;
const message: AdminMessage = {
  id: ID, org: orgRef, title: 'A change for members', body: '## Current message\n\nThe **accepted** Markdown.',
  sender: ADA, created_at: '2026-11-08T12:00:00Z', recipient_count: 4, delivery_job_id: null,
};
const page: AdminMessageListResponse = { items: [], next_cursor: null, audience_count: 8 };
const scopes = ['org', 'system'] as const;
const locales = ['en', 'zh-CN'] as const;
type Kind = typeof scopes[number];
const scopeOf = (kind: Kind): MessageScope => kind === 'org' ? { kind, id: 'acme', name: 'Acme' } : { kind };
const received = (kind: Kind): AdminMessage => ({ ...message, org: kind === 'org' ? orgRef : null });
const summary = (kind: Kind): AdminMessageSummary => ({
  id: message.id, org: received(kind).org, title: message.title, sender: message.sender,
  created_at: message.created_at, recipient_count: message.recipient_count, delivery_job_id: message.delivery_job_id,
});
const denied = () => new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', status: 403,
  title: 'Denied', detail: '', request_id: 'message-decision' }, null);
function held<T>() {
  let release: (value: T) => void = () => undefined;
  const promise = new Promise<T>((resolve) => { release = resolve; });
  return { promise, release: (value: T) => release(value) };
}
async function focusAgain() {
  await act(async () => { focusManager.setFocused(false); focusManager.setFocused(true); await Promise.resolve(); });
}
async function fill() {
  await userEvent.type(screen.getByRole('textbox', { name: i18n.t('adminMessages.title') }), 'One message');
  await userEvent.type(screen.getByRole('textbox', { name: i18n.t('adminMessages.body') }), 'Accepted local body');
}

beforeEach(() => {
  calls.canManageMembers = true;
  calls.send.mockReset().mockResolvedValue({ projection: 'receipt', message_id: ID, recipient_count: 4, delivery_job_id: null });
  calls.list.mockReset().mockResolvedValue(page);
  calls.read.mockReset().mockResolvedValue(message);
  calls.put.mockReset();
  calls.node.mockReset().mockImplementation((id) => {
    const value = org(id, 'acme', 'Acme', ADA);
    return Promise.resolve({ ...value, access: { ...value.access, owns: calls.canManageMembers,
      permissions: calls.canManageMembers ? ['node.view', 'members.manage'] : ['node.view'] } });
  });
});
afterEach(async () => { cleanup(); focusManager.setFocused(undefined); await setLocale('en'); });

for (const kind of scopes) it.each(locales)(`${kind} reads the accepted scope, sender, time and Markdown in %s`, async (locale) => {
  await setLocale(locale); calls.read.mockResolvedValue(received(kind));
  show(<AdministratorMessage id={ID} />);
  expect(await screen.findByRole('heading', { name: message.title })).toBeVisible();
  expect(screen.getByRole('heading', { name: 'Current message' })).toBeVisible();
  expect(screen.getByText('accepted', { exact: true })).toHaveProperty('tagName', 'STRONG');
  expect(screen.getByText(i18n.t(kind === 'org' ? 'adminMessages.fromOrg' : 'adminMessages.fromSystem', { org: 'Acme' }))).toBeVisible();
  expect(screen.getByText(ADA.display_name, { exact: true })).toBeVisible();
  expect(document.querySelector('time')).toHaveAttribute('dateTime', message.created_at);
  expect(calls.read).toHaveBeenCalledWith(ID);
});

it.each(locales)('retires a previously readable message after a current authority refusal in %s', async (locale) => {
  await setLocale(locale); calls.read.mockResolvedValueOnce(message).mockRejectedValue(denied());
  show(<AdministratorMessage id={ID} />);
  expect(await screen.findByRole('heading', { name: message.title })).toBeVisible();
  await focusAgain(); await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(2));
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(screen.queryByRole('heading', { name: message.title })).not.toBeInTheDocument();
  expect(screen.queryByText('accepted', { exact: true })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: i18n.t('common.tryAgain') })).not.toBeInTheDocument();
});

it.each(locales)('keeps a failed background message read dimmed and retries through its visible action in %s', async (locale) => {
  await setLocale(locale); calls.read.mockResolvedValueOnce(message).mockRejectedValue(new Error('The connection ended'));
  show(<AdministratorMessage id={ID} />);
  expect(await screen.findByRole('heading', { name: message.title })).toBeVisible();
  await focusAgain(); await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(2));
  const retry = await screen.findByRole('button', { name: i18n.t('common.tryAgain') });
  expect(screen.getByRole('heading', { name: message.title }).closest('.opacity-60')).not.toBeNull();
  calls.read.mockResolvedValue({ ...message, body: '## Current message\n\nRead again under current access.' });
  await userEvent.click(retry);
  expect(await screen.findByText('Read again under current access.')).toBeVisible();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
});

for (const kind of scopes) it.each(locales)(`${kind} pages the sent list in scope without embedding the full message in %s`, async (locale) => {
  await setLocale(locale);
  const first = summary(kind), older = { ...first, id: OLDER, title: 'Earlier accepted message', created_at: '2026-11-07T12:00:00Z' };
  calls.list.mockImplementation((_org, cursor) => Promise.resolve(cursor === null
    ? { ...page, items: [first], next_cursor: 'older-page' } : { ...page, items: [older] }));
  show(<MessagesSection scope={scopeOf(kind)} />);
  expect(await screen.findByRole('link', { name: first.title })).toHaveAttribute('href', `/messages/${ID}`);
  expect(screen.queryByText('Current message', { exact: true })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('ui.pagination.more') }));
  expect(await screen.findByRole('link', { name: older.title })).toHaveAttribute('href', `/messages/${OLDER}`);
  expect(calls.list).toHaveBeenCalledWith(kind === 'org' ? 'acme' : null, null);
  expect(calls.list).toHaveBeenCalledWith(kind === 'org' ? 'acme' : null, 'older-page');
  expect(screen.getAllByRole('link').filter((link) => link.getAttribute('href')?.startsWith('/messages/')).map((link) => link.textContent))
    .toEqual([first.title, older.title]);
  expect(calls.read).not.toHaveBeenCalled();
});

for (const kind of scopes) it.each(locales)(`${kind} retires denied sent rows and their stale audience preview in %s`, async (locale) => {
  await setLocale(locale); calls.list.mockResolvedValueOnce({ ...page, items: [summary(kind)], next_cursor: 'older-page' }).mockRejectedValue(denied());
  show(<MessagesSection scope={scopeOf(kind)} />);
  expect(await screen.findByRole('link', { name: message.title })).toBeVisible();
  await focusAgain(); await waitFor(() => expect(calls.list).toHaveBeenCalledTimes(2));
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(screen.queryByRole('link', { name: message.title })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: i18n.t('ui.pagination.more') })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: i18n.t(kind === 'org' ? 'adminMessages.messageMembers' : 'adminMessages.messageEveryone') }));
  await fill();
  expect(await screen.findByText(i18n.t(kind === 'org' ? 'adminMessages.sendsToMembers' : 'adminMessages.sendsToPeople', { number: '–', count: 0 }))).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('adminMessages.send') })).toBeDisabled();
  expect(calls.send).not.toHaveBeenCalled();
});

for (const kind of scopes) it.each(locales)(`${kind} keeps sent rows visible during a temporary read failure and repairs them in %s`, async (locale) => {
  await setLocale(locale); calls.list.mockResolvedValueOnce({ ...page, items: [summary(kind)] }).mockRejectedValue(new Error('The connection ended'));
  show(<MessagesSection scope={scopeOf(kind)} />);
  expect(await screen.findByRole('link', { name: message.title })).toBeVisible();
  await focusAgain(); await waitFor(() => expect(calls.list).toHaveBeenCalledTimes(2));
  const retry = await screen.findByRole('button', { name: i18n.t('common.tryAgain') });
  expect(screen.getByRole('link', { name: message.title }).closest('.opacity-60')).not.toBeNull();
  calls.list.mockResolvedValue({ ...page, items: [summary(kind)] });
  await userEvent.click(retry); await waitFor(() => expect(calls.list).toHaveBeenCalledTimes(3));
  await waitFor(() => expect(screen.getByRole('link', { name: message.title }).closest('.opacity-60')).toBeNull());
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
});

for (const kind of scopes) it.each(locales)(`${kind} keeps an unknown audience unknown and waits for the native preview in %s`, async (locale) => {
  await setLocale(locale); const result = held<AdminMessageListResponse>(); calls.list.mockReturnValue(result.promise);
  show(<MessageComposer scope={scopeOf(kind)} onClose={() => undefined} />); await fill();
  const send = screen.getByRole('button', { name: i18n.t('adminMessages.send') });
  try {
    expect(screen.getByText(i18n.t(kind === 'org' ? 'adminMessages.sendsToMembers' : 'adminMessages.sendsToPeople', { number: '–', count: 0 }))).toBeVisible();
    expect(send).toBeDisabled(); expect(calls.send).not.toHaveBeenCalled();
  } finally { await act(() => { result.release(page); return Promise.resolve(); }); }
  expect(await screen.findByText(i18n.t(kind === 'org' ? 'adminMessages.sendsToMembers' : 'adminMessages.sendsToPeople', { number: '8', count: 8 }))).toBeVisible();
  expect(send).toBeEnabled();
});

for (const kind of scopes) it.each(locales)(`${kind} keeps pending Send focused and accepts no second submission in %s`, async (locale) => {
  await setLocale(locale); const result = held<Awaited<ReturnType<typeof Messages.sendMessage>>>(); calls.send.mockReturnValue(result.promise);
  show(<MessageComposer scope={scopeOf(kind)} onClose={() => undefined} />); await fill();
  const send = screen.getByRole('button', { name: i18n.t('adminMessages.send') });
  await waitFor(() => expect(send).toBeEnabled());
  try {
    await userEvent.click(send);
    await waitFor(() => expect(send).toHaveAttribute('aria-busy', 'true'));
    expect(send).toHaveAttribute('aria-disabled', 'true'); expect(send).toHaveFocus();
    expect(screen.getByRole('textbox', { name: i18n.t('adminMessages.body') })).toBeDisabled();
    await userEvent.keyboard('{Enter}'); await userEvent.click(send);
    expect(calls.send).toHaveBeenCalledTimes(1);
  } finally { await act(() => { result.release({ projection: 'receipt', message_id: ID, recipient_count: 4, delivery_job_id: null }); return Promise.resolve(); }); }
  expect(await screen.findByText(i18n.t(kind === 'org' ? 'adminMessages.sentToMembers' : 'adminMessages.sentToPeople', { number: '4', count: 4 }))).toBeVisible();
});

it.each(locales)('keeps the system banner optional and requires an expiry only when selected in %s', async (locale) => {
  await setLocale(locale); show(<MessageComposer scope={{ kind: 'system' }} onClose={() => undefined} />); await fill();
  const banner = screen.getByRole('checkbox', { name: i18n.t('adminMessages.showBanner') });
  const send = screen.getByRole('button', { name: i18n.t('adminMessages.send') });
  expect(banner).not.toBeChecked(); expect(screen.queryByRole('textbox', { name: i18n.t('adminMessages.until') })).not.toBeInTheDocument();
  await userEvent.click(banner);
  expect(screen.getByRole('textbox', { name: i18n.t('adminMessages.until') })).toHaveValue(''); expect(send).toBeDisabled();
  await userEvent.click(banner); await waitFor(() => expect(send).toBeEnabled()); await userEvent.click(send);
  await waitFor(() => expect(calls.send).toHaveBeenCalledOnce());
  expect(calls.send.mock.calls[0]?.[0]).toBeNull(); expect(calls.send.mock.calls[0]?.[1]).not.toHaveProperty('banner_until');
});

const terms: SettingDef = { key: 'org.terms', title: 'Terms for people who join', description: 'Registry words', group: 'joining', class: 'policy',
  scopes: ['org'], merge: 'append', default: '', example: 'Keep private data here', schema: { type: 'string' }, text_key: null, keywords: [] };
const root = { id: 'root', kind: 'org', name: 'Root organization', redacted: false } as const;
for (const mode of ['read-only', 'inherited', 'locked'] as const) it.each(locales)(`${mode} terms keep the member-manager action outside the terms write in %s`, async (locale) => {
  await setLocale(locale); const submitted = vi.fn(), changed = vi.fn();
  const source = { scope: 'org', node: mode === 'inherited' ? root : orgRef } as const;
  const setting: ResolvedSetting = { key: 'org.terms', value: '# Current terms', own: mode === 'inherited' ? null : { value: '# Current terms', revision: 3, locked: false },
    source, locked_by: mode === 'locked' ? { scope: 'org', node: root } : null, writable: false,
    parts: [{ source, value: '# Current terms', locked: mode === 'locked', ignored: false }] };
  show(<form onSubmit={(event) => { event.preventDefault(); submitted(); }}><ul><SettingRow def={withGroup(localizedSetting(terms, i18n))} setting={setting}
    target={{ kind: 'node', id: 'acme' }} place={{ name: 'Acme', owner: null, access: { permissions: ['members.manage'] }, kind: 'org' }} onChanged={changed} /></ul></form>);
  await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('adminMessages.tellMembers') })).not.toHaveAttribute('aria-disabled', 'true'));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('adminMessages.tellMembers') }));
  expect(await screen.findByRole('textbox', { name: i18n.t('adminMessages.title') })).toHaveValue(i18n.t('adminMessages.termsChanged', { org: 'Acme' }));
  expect(screen.getByRole('textbox', { name: i18n.t('adminMessages.body') })).toHaveValue('');
  expect(submitted).not.toHaveBeenCalled(); expect(changed).not.toHaveBeenCalled(); expect(calls.put).not.toHaveBeenCalled(); expect(calls.send).not.toHaveBeenCalled();
});

it.each(locales)('keeps the terms message action disabled with its current member-management reason in %s', async (locale) => {
  await setLocale(locale); calls.canManageMembers = false; show(<TellMembers id="acme" />);
  await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('adminMessages.tellMembers') })).toHaveAttribute('aria-description', i18n.t('implicitAvailability.membersManage')));
  const tell = screen.getByRole('button', { name: i18n.t('adminMessages.tellMembers') });
  expect(tell).toHaveAttribute('aria-disabled', 'true');
  await userEvent.click(tell);
  expect(screen.queryByRole('textbox', { name: i18n.t('adminMessages.title') })).not.toBeInTheDocument();
  expect(calls.node).toHaveBeenCalledWith('acme'); expect(calls.send).not.toHaveBeenCalled();
});
