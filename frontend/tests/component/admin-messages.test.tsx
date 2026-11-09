/** Both native scopes share the mounted composer, its retry key and current accepted count. */
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Messages from '@/api/adminMessages';
import type * as Nodes from '@/api/nodes';
import { MessageComposer } from '@/features/admin-messages/Composer';
import { MessagesSection } from '@/features/admin-messages/Sent';
import { AdministratorMessage } from '@/features/admin-messages/Message';
import { TellMembers } from '@/features/admin-messages/TellMembers';
import { notificationText } from '@/features/inbox/text';
import type { AdminMessage, AdminMessageListResponse, Node, Notification } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { org } from '../doubles/fixtures';
import { ADA } from '../unit/general-access-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ manage: true, send: vi.fn<typeof Messages.sendMessage>(),
  list: vi.fn<(org: string | null) => Promise<AdminMessageListResponse>>(),
  read: vi.fn<(id: string) => Promise<AdminMessage>>() }));
vi.mock('@/api/adminMessages', () => ({
  sendMessage: calls.send,
  sentMessagesQuery: (org: string | null) => ({ queryKey: ['admin-messages', org ?? 'system'], initialPageParam: null,
    queryFn: () => calls.list(org), getNextPageParam: (page: AdminMessageListResponse) => page.next_cursor }),
  messageQuery: (id: string) => ({ queryKey: ['admin-message', id], queryFn: () => calls.read(id) }),
}));
vi.mock('@/api/nodes', async (load) => ({ ...await load<typeof Nodes>(), nodeQuery: (id: string) => ({ queryKey: ['message-org', id], queryFn: () => {
  const value = org(id, 'acme', 'Acme', ADA);
  return Promise.resolve({ ...value, access: { ...value.access, owns: calls.manage, permissions: calls.manage ? ['node.view', 'members.manage'] : ['node.view'] } });
} }) }));

const ID = '00000000-0000-4000-8000-000000000321';
const at = '2026-11-08T12:00:00Z';
const message: AdminMessage = { id: ID, org: { id: 'acme', kind: 'org', name: 'Acme', redacted: false },
  title: 'A change for members', body: '## What changed\n\nPlease read this **important** change.',
  sender: ADA, created_at: at, recipient_count: 4, delivery_job_id: null };
const page: AdminMessageListResponse = { items: [], next_cursor: null, audience_count: 8 };
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const before = useAppearance.getState();
beforeEach(() => { calls.manage = true; calls.send.mockReset(); calls.list.mockReset().mockResolvedValue(page); calls.read.mockReset().mockResolvedValue(message); });
afterEach(async () => { cleanup(); useAppearance.setState(before); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('keeps Tell members visible when its current Manage door is refused in %s', async (locale) => {
  await setLocale(locale); calls.manage = false;
  show(<TellMembers id="acme" />);
  const button = screen.getByRole('button', { name: i18n.t('adminMessages.tellMembers') });
  await waitFor(() => expect(button).toHaveAttribute('aria-description', i18n.t('implicitAvailability.membersManage')));
  expect(button).toHaveAttribute('aria-disabled', 'true');
  await userEvent.click(button);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(calls.send).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('keeps the same message draft and masks the audience when current Manage ends in %s', async (locale) => {
  await setLocale(locale); const user = userEvent.setup();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  show(<QueryClientProvider client={client}><TellMembers id="acme" /></QueryClientProvider>);
  await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('adminMessages.tellMembers') })).not.toHaveAttribute('aria-disabled', 'true'));
  await user.click(screen.getByRole('button', { name: i18n.t('adminMessages.tellMembers') }));
  const dialog = screen.getByRole('dialog');
  const body = within(dialog).getByRole('textbox', { name: i18n.t('adminMessages.body') });
  await user.click(body); await user.paste('Keep this member message');
  const current = client.getQueryData<Node>(['message-org', 'acme']);
  if (current === undefined) throw new Error('the native org has been loaded');
  act(() => { client.setQueryData(['message-org', 'acme'], { ...current, access: { ...current.access, permissions: ['node.view'] } }); });
  await waitFor(() => expect(within(dialog).getByRole('button', { name: i18n.t('adminMessages.send') })).toHaveAttribute('aria-description', i18n.t('implicitAvailability.membersManage')));
  expect(body).toHaveValue('Keep this member message');
  const form = dialog.querySelector('form');
  if (form === null) throw new Error('the native message composer has its form');
  fireEvent.submit(form);
  expect(calls.send).not.toHaveBeenCalled();
});

for (const kind of ['org', 'system'] as const) it.each(modes)(`${kind} preserves failed text and key, previews Markdown, and uses the accepted count in %s/%s`, async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  calls.send.mockRejectedValueOnce(new Error('The answer was lost')).mockResolvedValue({ projection: 'receipt', message_id: ID, recipient_count: 4, delivery_job_id: null });
  const scope = kind === 'org' ? { kind, id: 'acme', name: 'Acme' } : { kind };
  show(<MessageComposer scope={scope} onClose={() => undefined} />);
  await userEvent.type(screen.getByRole('textbox', { name: i18n.t('adminMessages.title') }), 'A change');
  await userEvent.type(screen.getByRole('textbox', { name: i18n.t('adminMessages.body') }), '## Native preview');
  await userEvent.click(screen.getByRole('tab', { name: i18n.t('adminMessages.preview') }));
  expect(screen.getByRole('heading', { name: 'Native preview' })).toBeVisible();
  await userEvent.click(screen.getByRole('tab', { name: i18n.t('adminMessages.write') }));
  expect(screen.getByRole('textbox', { name: i18n.t('adminMessages.body') })).toHaveValue('## Native preview');
  if (kind === 'system') {
    await userEvent.click(screen.getByRole('checkbox', { name: i18n.t('adminMessages.showBanner') }));
    await userEvent.type(screen.getByRole('textbox', { name: i18n.t('adminMessages.until') }), '2027-01-02 12:30');
  } else expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
  expect(await screen.findByText(i18n.t(kind === 'org' ? 'adminMessages.sendsToMembers' : 'adminMessages.sendsToPeople', { number: '8' }))).toBeVisible();
  const send = screen.getByRole('button', { name: i18n.t('adminMessages.send') });
  await waitFor(() => expect(send).toBeEnabled());
  await userEvent.click(send);
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(screen.getByRole('textbox', { name: i18n.t('adminMessages.body') })).toHaveValue('## Native preview');
  await userEvent.click(send);
  expect(await screen.findByText(i18n.t(kind === 'org' ? 'adminMessages.sentToMembers' : 'adminMessages.sentToPeople', { number: '4' }))).toBeVisible();
  expect(calls.send).toHaveBeenCalledTimes(2);
  expect(calls.send.mock.calls[1]).toEqual(calls.send.mock.calls[0]);
  expect(calls.send.mock.calls[0]?.[2]).toMatch(/^[a-f0-9-]{36}$/i);
  expect(calls.send.mock.calls[0]?.[0]).toBe(kind === 'org' ? 'acme' : null);
  if (kind === 'system') expect(calls.send.mock.calls[0]?.[1].banner_until).toBe(new Date('2027-01-02T12:30').toISOString());
  expect(screen.queryByRole('button', { name: i18n.t('adminMessages.send') })).not.toBeInTheDocument();
});

it.each(['en', 'zh-CN'] as const)('opens the terms composer with the native org title and keeps its body local in %s', async (locale) => {
  await setLocale(locale);
  show(<TellMembers id="acme" />);
  await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('adminMessages.tellMembers') })).not.toHaveAttribute('aria-disabled', 'true'));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('adminMessages.tellMembers') }));
  expect(await screen.findByRole('textbox', { name: i18n.t('adminMessages.title') })).toHaveValue(i18n.t('adminMessages.termsChanged', { org: 'Acme' }));
  expect(screen.getByRole('textbox', { name: i18n.t('adminMessages.body') })).toHaveValue('');
  expect(calls.send).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('shows a body-free sent row and opens the same received message reader in %s', async (locale) => {
  await setLocale(locale);
  const summary = { id: message.id, org: message.org, title: message.title, sender: message.sender,
    created_at: message.created_at, recipient_count: message.recipient_count, delivery_job_id: message.delivery_job_id };
  calls.list.mockResolvedValue({ ...page, items: [summary] });
  const view = show(<MessagesSection scope={{ kind: 'org', id: 'acme', name: 'Acme' }} />);
  expect(await screen.findByRole('link', { name: message.title })).toHaveAttribute('href', `/messages/${ID}`);
  expect(screen.queryByText('important')).not.toBeInTheDocument();
  view.unmount();
  show(<AdministratorMessage id={ID} />);
  expect(await screen.findByRole('heading', { name: message.title })).toBeVisible();
  expect(screen.getByRole('heading', { name: 'What changed' })).toBeVisible();
  expect(screen.getByText(i18n.t('adminMessages.fromOrg', { org: 'Acme' }))).toBeVisible();
  expect(screen.getByText('important')).toHaveProperty('tagName', 'STRONG');
});

it.each(['en', 'zh-CN'] as const)('keeps the Inbox message compact and attributes its accepted scope in %s', async (locale) => {
  await setLocale(locale);
  const row: Notification = { id: 'notice', kind: 'admin_message', title: message.title, body: message.body,
    node: null, thread_id: null, approval_id: null, chat_id: null, url: `/messages/${ID}`,
    data: { org_id: 'acme', org_name: 'Acme' }, created_at: at, read_at: null,
    inbox_seq: 1, group: null, waiting_for_me: false };
  expect(notificationText(row, i18n.t, locale)).toEqual({ title: message.title, body: i18n.t('adminMessages.fromOrg', { org: 'Acme' }) });
  expect(notificationText({ ...row, data: { org_id: null, org_name: null } }, i18n.t, locale).body).toBe(i18n.t('adminMessages.fromSystem'));
});


it.each(modes)('keeps received markup as written data in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  calls.read.mockResolvedValue({ ...message, body: '## Reader formatting\n\n<aside data-message-note="literal">Written words</aside>\n\n![Diagram](https://diagram.example.invalid/example.png)\n\n[A note](note:example)' });
  const view = show(<AdministratorMessage id={ID} />);
  expect(await screen.findByRole('heading', { name: 'Reader formatting' })).toBeVisible();
  expect(screen.getByText('<aside data-message-note="literal">Written words</aside>', { exact: true })).toBeVisible();
  expect(view.container.querySelector('[data-message-note]')).toBeNull();
  expect(screen.getByRole('link', { name: 'Diagram' })).toHaveAttribute('href', 'https://diagram.example.invalid/example.png');
  expect(screen.queryByRole('img', { name: 'Diagram' })).toBeNull();
  expect(screen.getByText('A note', { exact: true })).toBeVisible();
  expect(screen.queryByRole('link', { name: 'A note' })).toBeNull();
});
