/** UA-16: current thread identity, one event, supplied private-safe context and no labeling requests. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type * as Router from '@tanstack/react-router';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { orgsQuery } from '@/api/nodes';
import { NotificationRow } from '@/features/inbox/NotificationRow';
import { useNotificationLabels } from '@/features/inbox/presentation';
import type { NodeRef, Notification, UserRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { userWriter, useUserStore } from '@/realtime/stores/user';
import { org } from '../doubles/fixtures';
import { ADA } from '../unit/general-access-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ navigate: vi.fn<(_target: { href: string }) => Promise<void>>(), read: vi.fn<(_body: unknown) => Promise<void>>() }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(), useNavigate: () => calls.navigate }));
vi.mock('@/features/inbox/read', () => ({ markNotificationsRead: calls.read }));
vi.mock('@/ui/UserChip',()=>({UserChip:({user}:{user:UserRef})=><button data-person-chip={user.id}>{user.display_name}</button>}));
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const before = useAppearance.getState();
const wasDark = document.documentElement.classList.contains('dark');
const previousTheme = document.documentElement.dataset.theme;
const ref = (id: string, kind: NodeRef['kind'], name: string): NodeRef => ({ id, kind, name, redacted: false });
function notice(id: string, title: string, kind: Notification['kind'] = 'turn.completed', root = 'Acme'): Notification {
  const thread = ref(id, 'thread', title);
  return { id: `notice-${id}`, kind, title: 'Stale stored thread name', body: 'Turn finished',
    thread_id: id, thread, node: ref(`${root}-topic`, 'topic', 'Login'),
    path: [ref(root, 'org', root), ref(`${root}-project`, 'project', 'Web'), ref(`${root}-topic`, 'topic', 'Login'), thread],
    approval_id: null, chat_id: null, url: `/thread/${id}`, data: {},
    details: { v: 1, action: kind, parameters: { subject: 'Stale stored thread name' } },
    created_at: '2026-10-15T16:00:00Z', read_at: null, inbox_seq: 1, group: null, waiting_for_me: false,
  };
}
function Rows({ rows, byPlace = false }: { rows: readonly Notification[]; byPlace?: boolean }) {
  const labels = useNotificationLabels(rows);
  return <ul>{rows.map((row) => <NotificationRow key={row.id} row={row} byPlace={byPlace} labels={labels} />)}</ul>;
}
beforeEach(() => { vi.clearAllMocks(); userWriter.reset([], { unread: 0, approvalsPending: 0 }, 0, null); calls.navigate.mockResolvedValue(undefined); calls.read.mockResolvedValue(undefined); });
afterEach(async () => {
  cleanup(); vi.restoreAllMocks(); useAppearance.setState(before);
  userWriter.reset([], { unread: 0, approvalsPending: 0 }, 0, null);
  document.documentElement.classList.toggle('dark', wasDark);
  if (previousTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = previousTheme;
  await setLocale('en');
});

it('keeps the sender of an explicit person notice independently reachable', () => {
  const kind = 'person.notify' as unknown as Notification['kind'];
  const row = notice('explicit', 'Guide', kind);
  row.actor = ADA;
  show(<Rows rows={[row]} />);
  const item = screen.getByRole('listitem');
  const sender = within(item).getByRole('button', { name: ADA.display_name });
  expect(sender).toBeVisible();
  expect(within(item).getByRole('link')).not.toContainElement(sender);
});

it.each(modes)('names distinct threads in one topic and one event/context without fetching navigation in %s/%s', async (locale, theme) => {
  await setLocale(locale);
  useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.classList.toggle('dark', theme === 'dark');
  document.documentElement.dataset.theme = theme;
  const fetch = vi.spyOn(globalThis, 'fetch').mockRejectedValue(new Error('Inbox labels need no request'));
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(orgsQuery.queryKey, [org('Acme', 'acme', 'Acme', ADA), org('Beta', 'beta', 'Beta', ADA)]);
  const rows = [notice('redirect', 'Fix the redirect'), notice('limits', 'Check the rate limit', 'thread.blocked'), notice('other', 'Release the change', 'turn.completed', 'Beta')];
  show(<QueryClientProvider client={cache}><Rows rows={rows} /></QueryClientProvider>);
  const shown = screen.getAllByRole('listitem');
  for (const [index, row] of rows.entries()) {
    const item = shown[index];
    if (item === undefined) throw new Error('every supplied notification is drawn');
    expect(item.querySelector('[data-inbox-title]')).toHaveTextContent(row.thread?.name ?? '');
    expect(within(item).getAllByText(row.thread?.name ?? '')).toHaveLength(1);
    const event = i18n.t(row.kind === 'thread.blocked' ? 'inbox.actions.threadBlocked' : 'inbox.actions.turnCompleted');
    expect(item.querySelector('[data-inbox-event]')).toHaveTextContent(event);
    expect(within(item).getAllByText(event)).toHaveLength(1);
    expect(item.querySelector('[data-inbox-place]')).toHaveTextContent(index === 2 ? 'Login · Beta' : 'Login · Acme');
    expect(within(item).queryByText('Stale stored thread name')).not.toBeInTheDocument();
    expect(within(item).queryByText('Turn finished')).not.toBeInTheDocument();
    expect(item.querySelector('.truncate, .line-clamp-2')).toBeNull();
  }
  const first = shown[0];
  if (first === undefined) throw new Error('a first row');
  await userEvent.click(within(first).getByRole('link'));
  expect(calls.read).toHaveBeenCalledWith({ ids: ['notice-redirect'] });
  expect(calls.navigate).toHaveBeenCalledWith({ href: '/thread/redirect' });
  expect(fetch).not.toHaveBeenCalled();
  cache.clear();
});

it.each(modes)('keeps hidden names hidden and omits context already given by its group in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.classList.toggle('dark', theme === 'dark'); document.documentElement.dataset.theme = theme;
  const hidden = notice('private', 'Forbidden old name');
  hidden.thread = { id: 'private', kind: 'thread', name: null, redacted: true };
  hidden.path = [ref('Acme', 'org', 'Acme'), { id: 'secret', kind: 'topic', name: null, redacted: true }, hidden.thread];
  hidden.node = { id: 'secret', kind: 'topic', name: null, redacted: true };
  const grouped = notice('grouped', 'Write the guide');
  grouped.group = { key: 'Login', place: grouped.node, unread: 1, waiting_for_me: 0, through_seq: 1 };
  show(<Rows rows={[hidden, grouped]} byPlace />);
  expect(screen.getByText(i18n.t('inbox.row.privateThread'))).toBeVisible();
  expect(screen.queryByText('Forbidden old name')).not.toBeInTheDocument();
  expect(screen.queryByText('Stale stored thread name')).not.toBeInTheDocument();
  const groupedItem = screen.getByText('Write the guide').closest('li');
  if (groupedItem === null) throw new Error('the named row');
  expect(groupedItem.querySelector('[data-inbox-place]')).toBeNull();
  expect(groupedItem.querySelector('[data-inbox-event]')).toHaveTextContent(i18n.t('inbox.actions.turnCompleted'));
});

it.each(modes)('names the requester once and opens that request itself in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.classList.toggle('dark', theme === 'dark'); document.documentElement.dataset.theme = theme;
  const row = notice('review', 'Review the release', 'access.requested');
  row.actor = { ...ADA, id: 'ben', username: 'ben', address: 'acme.ben', display_name: 'Ben' };
  row.body = '';
  row.approval_id = 'the-exact-request';
  row.url = '/approvals?approval=the-exact-request';
  show(<Rows rows={[row]} />);
  const item = screen.getByRole('listitem');
  expect(item.querySelector('[data-inbox-title]')).toHaveTextContent('Review the release');
  const event = i18n.t('inbox.actions.accessRequestedBy', { name: 'Ben' });
  expect(item.querySelector('[data-inbox-event]')).toHaveTextContent(event);
  expect(item.querySelectorAll('[data-inbox-event]')).toHaveLength(1);
  const requester = within(item).getByRole('button', { name: 'Ben' });
  expect(requester).toHaveAttribute('data-person-chip', row.actor.id);
  expect(requester.closest('a')).toBeNull();
  expect(item.textContent.match(/Ben/g)).toHaveLength(1);
  expect(item).not.toHaveTextContent('acme.ben');
  await userEvent.click(within(item).getByRole('link'));
  expect(calls.navigate).toHaveBeenCalledWith({ href: row.url });
  expect(calls.read).toHaveBeenCalledWith({ ids: [row.id] });
});

function LiveRows() {
  const rows = useUserStore((state) => state.rows);
  return <Rows rows={[...rows.values()]} />;
}

it.each(modes)('replaces the waiting request with its actual decision and decider on the same live row in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme;
  const row = notice('decision', 'Review the release', 'approval.requested');
  row.approval_id = 'exact-approval'; row.waiting_for_me = true; row.body = 'Run ./deploy staging';
  userWriter.reset([row], { unread: 1, approvalsPending: 1 }, 1, null);
  show(<LiveRows />);
  const item = screen.getByRole('listitem');
  expect(within(item).getByText(i18n.t('inbox.actions.approvalRequested'))).toBeVisible();
  const decider = { ...ADA, display_name: 'Ada Lovelace' };
  act(() => userWriter.apply([{
    ...row, actor: decider, waiting_for_me: false, read_at: '2026-10-15T16:01:00Z', inbox_seq: 2,
    data: { approval_state: 'approved', decided_by: decider.id },
    details: { v: 1, action: row.kind, parameters: { approval_state: 'approved', decided_by: decider.id } },
  }], { unread: 0, approvalsPending: 0 }, 2));
  expect(screen.getByRole('listitem')).toBe(item);
  expect(within(item).getByRole('status')).toHaveTextContent(i18n.t('inbox.actions.approvalAllowed'));
  const person=within(item).getByRole('button',{name:'Ada Lovelace'});expect(person).toHaveAttribute('data-person-chip',decider.id);expect(person.closest('a')).toBeNull();
  expect(item.textContent.match(/Ada Lovelace/gu)).toHaveLength(1);
  expect(item.querySelector('[data-inbox-title]')).toHaveTextContent('Review the release');
  expect(within(item).getByText('Run ./deploy staging')).toBeVisible();
  expect(within(item).queryByText(i18n.t('inbox.actions.approvalRequested'))).not.toBeInTheDocument();
  expect(within(item).queryByText(i18n.t('away.inbox.waiting'))).not.toBeInTheDocument();
  expect(item).toHaveAttribute('data-unread', 'false');
  expect(useUserStore.getState().counters).toEqual({ unread: 0, approvalsPending: 0 });
  expect(calls.read).not.toHaveBeenCalled();
});

it.each(modes)('gives missing and blank current names a useful title without borrowing a stored private name in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme;
  const missing = notice('missing', 'Old name'); missing.thread = null;
  const blank = notice('blank', '  ');
  const generic = notice('generic', 'Thread');
  show(<Rows rows={[missing, blank, generic]} />);
  const items = screen.getAllByRole('listitem');
  expect(items).toHaveLength(3);
  for (const item of items.slice(0, 2)) {
    expect(item.querySelector('[data-inbox-title]')).toHaveTextContent(i18n.t('inbox.row.thread'));
    expect(item.querySelector('[data-inbox-event]')).toHaveTextContent(i18n.t('inbox.actions.turnCompleted'));
    expect(item).not.toHaveTextContent('Stale stored thread name');
    expect(item.querySelector('[data-inbox-title]')?.textContent).not.toMatch(/^(?:Unread: )?(?:Thread|会话)$/u);
  }
  const item = items.at(2);
  if (item === undefined) throw new Error('the generic-name notice remains');
  expect(item.querySelector('[data-inbox-title]')).toHaveTextContent(i18n.t('inbox.row.threadInPlace', { thread: 'Thread', place: 'Login' }));
  expect(item.querySelector('[data-inbox-place]')).toBeNull();
  expect(item.querySelector('[data-inbox-event]')).toHaveTextContent(i18n.t('inbox.actions.turnCompleted'));
});

it.each(modes)('keeps a settled source outcome, its UserChip and its actual decision time outside the target link in %s/%s',async(locale,theme)=>{
  await setLocale(locale);useAppearance.setState({theme,resolvedTheme:theme});
  const row=notice('held-source','Current work','approval.requested');row.actor=ADA;row.read_at='2026-10-15T16:02:00Z';
  const parameters={approval_state:'approved',request_action:'run',decided_at:'2026-10-15T16:02:00Z',request_decision:{run:true}};
  row.data=parameters;row.details={v:1,action:row.kind,parameters};
  const view=show(<ul><NotificationRow row={row}/></ul>);
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('approvals.source.run'));
  const person=view.container.querySelector(`[data-person-chip="${ADA.id}"]`);expect(person).toBeInTheDocument();expect(person?.closest('a')).toBeNull();
  expect(screen.getByRole('status').querySelector('time')).toHaveAttribute('datetime','2026-10-15T16:02:00Z');
  expect(view.container).not.toHaveTextContent('Stale stored thread name');
});

it.each(modes)('marks only the actor slot when the target name contains the same name in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  const row = notice('matching-name', 'Unused', 'doc.comment');
  row.thread = null; row.thread_id = null; row.actor = ADA;
  row.details = { v: 1, action: 'doc.comment', parameters: { subject: `${ADA.display_name} guide` } };
  show(<Rows rows={[row]} />);
  const title = screen.getByRole('listitem').querySelector('[data-inbox-title]');
  expect(title).toHaveTextContent(`${ADA.display_name} guide`);
  expect(title?.querySelectorAll('[data-person-chip]')).toHaveLength(1);
  const actor = screen.getByRole('button', { name: ADA.display_name });
  expect(actor.closest('a')).toBeNull();
});
