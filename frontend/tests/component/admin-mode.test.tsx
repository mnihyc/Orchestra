/** The title-bar mode follows the human session, and a refusal never offers a self-grant step. */
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { applyChanges } from '@/api/acts';
import { systemKey, systemQuery } from '@/api/admin';
import { ApiError, client } from '@/api/client';
import { AdminModeControl, useAdminMode, useManagementAvailable } from '@/features/access/AdminMode';
import { AdminReachNotice } from '@/features/access/AdminReachNotice';
import { NoAccess } from '@/features/access/NoAccess';
import { AllChats } from '@/features/chat/AllChats';
import { Management } from '@/features/admin/Management';
import { ResourceNav } from '@/features/resources/ResourceNav';
import { TraceActor } from '@/features/trace/Requests';
import type { AdminMode, Chat, Me } from '@/generated/api-types';
import type { ChatListFilter } from '@/api/chats';
import { i18n, setLocale } from '@/i18n';
import { TooltipProvider } from '@/ui/Tooltip';

const calls = vi.hoisted(() => ({
  read: vi.fn<() => Promise<AdminMode>>(),
  write: vi.fn<(enabled: boolean) => Promise<AdminMode>>(),
  me: vi.fn<() => Promise<Pick<Me, 'is_system_admin'>>>(),
  chats: vi.fn<(root: string, filter: ChatListFilter) => Promise<{ items: Pick<Chat, 'id' | 'name'>[]; next_cursor: null }>>(),
}));
vi.mock('@/api/adminMode', () => ({
  adminModeKey: ['admin-mode'],
  adminModeQuery: { queryKey: ['admin-mode'], queryFn: () => calls.read() },
  setAdminMode: (enabled: boolean) => calls.write(enabled),
}));
vi.mock('@/api/auth', () => ({ meQuery: { queryKey: ['me'], queryFn: () => calls.me() } }));
vi.mock('@tanstack/react-router', () => ({
  useRouter: () => ({ invalidate: () => Promise.resolve() }),
  useNavigate: () => vi.fn(),
  useLocation: (options?: { select: (location: { pathname: string }) => string }) => options?.select === undefined ? { pathname: '/resources/machines' } : options.select({ pathname: '/resources/machines' }),
  Link: ({ to, className, children }: { to: string; className?: string; children?: ReactNode }) => <a href={to} className={className}>{children}</a>,
}));
vi.mock('@/shell/screen', () => ({ useScreenHeading: vi.fn() }));
vi.mock('@/features/admin/Overview', () => ({ Overview: () => <p>System overview</p> }));
vi.mock('@/features/navigation/placeLabels', () => ({ usePlaceLabel: () => (of: { name: string | null; redacted?: boolean }) => of.redacted === true ? null : of.name }));
vi.mock('@/api/chats', () => ({ orgChatsQuery: (root: string, filter: ChatListFilter) => ({
  queryKey: ['chats', root, filter], initialPageParam: null, queryFn: () => calls.chats(root, filter), getNextPageParam: () => undefined,
}) }));
vi.mock('@/features/chat/ChatRow', () => ({ ChatRow: ({ chat }: { chat: Pick<Chat, 'id' | 'name'> }) => <li><a href={`/chat/${chat.id}`}>{chat.name}</a></li> }));

let cache: QueryClient;
let state: AdminMode;
const ADMIN_MODE_NAME = /^As admin: (Off|On)$/;
const before = client.session.getState();
function draw(children: ReactNode) {
  const clientForView = cache;
  const Surround = ({ children: content }: { children: ReactNode }) => <QueryClientProvider client={clientForView}><TooltipProvider>{content}</TooltipProvider></QueryClientProvider>;
  return render(children, { wrapper: Surround });
}
function ManagementEntry() {
  return useManagementAvailable() ? <a href="/admin/overview">Management</a> : null;
}
beforeEach(async () => {
  vi.clearAllMocks();
  await setLocale('en');
  cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  state = { available: true, as_admin: false };
  calls.read.mockImplementation(() => Promise.resolve({ ...state }));
  calls.me.mockResolvedValue({ is_system_admin: true });
  calls.write.mockImplementation(async (enabled) => {
    state = { ...state, as_admin: enabled };
    await applyChanges([{ session: true, mode: { ...state } }], cache);
    return { ...state };
  });
  calls.chats.mockResolvedValue({ items: [{ id: 'readable-chat', name: 'A readable conversation' }], next_cursor: null });
  act(() => { client.session.setState({ status: 'signed_in', userId: 'human-session', accessEndsAt: null }); });
});
afterEach(() => {
  cache.clear();
  act(() => { client.session.setState(before); });
});

describe('the one human-session admin mode', () => {
  it('is absent when the server says this person administers nothing', async () => {
    state = { available: false, as_admin: false };
    draw(<AdminModeControl />);
    await waitFor(() => expect(calls.read).toHaveBeenCalledOnce());
    expect(screen.queryByRole('button', { name: ADMIN_MODE_NAME })).not.toBeInTheDocument();
    expect(screen.queryByRole('switch')).not.toBeInTheDocument();
  });

  it('stays off until confirmed and keeps the system Management destination available in either session state', async () => {
    const user = userEvent.setup();
    draw(<><AdminModeControl /><ManagementEntry /></>);
    const control = await screen.findByRole('switch', { name: ADMIN_MODE_NAME });
    expect(control).toHaveAttribute('data-as-admin', 'false');
    expect(screen.getByText('As admin: Off')).toBeVisible();
    expect(control).toHaveAccessibleName('As admin: Off');
    expect(control).not.toBeChecked();
    expect(screen.queryByRole('button', { name: ADMIN_MODE_NAME })).not.toBeInTheDocument();
    expect(calls.write).not.toHaveBeenCalled();
    expect(await screen.findByRole('link', { name: 'Management' })).toBeVisible();
    await user.click(control);
    await waitFor(() => expect(screen.getByRole('switch', { name: ADMIN_MODE_NAME })).toBeChecked());
    expect(screen.getAllByRole('switch', { name: ADMIN_MODE_NAME })).toHaveLength(1);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.getByText('As admin: On')).toBeVisible();
    expect(screen.getByRole('switch', { name: ADMIN_MODE_NAME })).toHaveAccessibleName('As admin: On');
    expect(calls.write).toHaveBeenLastCalledWith(true);
    expect(await screen.findByRole('link', { name: 'Management' })).toBeVisible();
    screen.getByRole('switch', { name: ADMIN_MODE_NAME }).focus();
    await user.keyboard(' ');
    await waitFor(() => expect(screen.getByRole('switch', { name: ADMIN_MODE_NAME })).not.toBeChecked());
    expect(screen.getByRole('link', { name: 'Management' })).toBeVisible();
    expect(calls.write).toHaveBeenLastCalledWith(false);
    expect(screen.getByRole('switch', { name: ADMIN_MODE_NAME })).not.toBeChecked();
  });

  it('offers the mode to a department admin while never offering system Management to them', async () => {
    state = { available: true, as_admin: true };
    calls.me.mockResolvedValue({ is_system_admin: false });
    draw(<><AdminModeControl /><ManagementEntry /></>);
    expect(await screen.findByRole('switch', { name: ADMIN_MODE_NAME })).toBeChecked();
    await waitFor(() => expect(calls.me).toHaveBeenCalledOnce());
    expect(screen.queryByRole('link', { name: 'Management' })).not.toBeInTheDocument();
  });

  it('keeps the confirmed state and keyboard focus while the one direct write is pending', async () => {
    let finish: () => void = () => { throw new Error('the pending write was not installed'); };
    const pending = new Promise<void>((resolve) => { finish = resolve; });
    calls.write.mockImplementationOnce(async (enabled) => {
      await pending;
      state = { ...state, as_admin: enabled };
      await applyChanges([{ session: true, mode: { ...state } }], cache);
      return { ...state };
    });
    const user = userEvent.setup();
    draw(<AdminModeControl />);
    const control = await screen.findByRole('switch', { name: 'As admin: Off' });
    control.focus();
    await user.keyboard(' ');
    expect(control).toHaveAttribute('aria-busy', 'true');
    expect(control).toHaveAttribute('aria-disabled', 'true');
    expect(control).toHaveFocus();
    expect(control).not.toBeChecked();
    await user.click(control);
    expect(calls.write).toHaveBeenCalledExactlyOnceWith(true);
    await act(async () => { finish(); await pending; });
    await waitFor(() => expect(screen.getByRole('switch', { name: ADMIN_MODE_NAME })).toBeChecked());
    expect(screen.getByRole('switch', { name: ADMIN_MODE_NAME })).toHaveAccessibleName('As admin: On');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('keeps the confirmed off state when changing the session is refused', async () => {
    const user = userEvent.setup();
    calls.write.mockRejectedValue(new Error('refused'));
    draw(<AdminModeControl />);
    await user.click(await screen.findByRole('switch', { name: ADMIN_MODE_NAME }));
    await waitFor(() => expect(calls.write).toHaveBeenCalledOnce());
    expect(screen.getByRole('switch', { name: ADMIN_MODE_NAME })).toHaveAttribute('data-as-admin', 'false');
    expect(screen.getByRole('switch', { name: ADMIN_MODE_NAME })).not.toBeChecked();
  });
});

it.each(['en', 'zh-CN'] as const)('names the confirmed off state on the direct switch in %s', async (locale) => {
  await setLocale(locale);
  draw(<AdminModeControl />);
  const label = i18n.t('adminReach.mode.label', { state: i18n.t('adminReach.mode.stateOff') });
  const control = await screen.findByRole('switch', { name: label });
  expect(screen.getByText(label)).toBeVisible();
  expect(control).toHaveAccessibleName(label);
  expect(control).not.toBeChecked();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(calls.write).not.toHaveBeenCalled();
});

it('offers Management’s off-state action on one line without another session switch', async () => {
  draw(<Management section="overview" search={{}} onSection={vi.fn()} onSearch={vi.fn()} />);
  expect(await screen.findByRole('heading', { level: 1, name: 'Management' })).toBeVisible();
  expect(await screen.findByRole('button', { name: 'Turn on As admin' })).toBeVisible();
  expect(screen.queryByRole('button', { name: ADMIN_MODE_NAME })).not.toBeInTheDocument();
  expect(screen.queryByRole('switch', { name: ADMIN_MODE_NAME })).not.toBeInTheDocument();
  expect(calls.write).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('opens Management with one enable act while the top bar keeps the only switch in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  draw(<><AdminModeControl /><Management section="overview" search={{}} onSection={vi.fn()} onSearch={vi.fn()} /></>);
  const enable = await screen.findByRole('button', { name: i18n.t('adminReach.mode.enable') });
  expect(screen.queryByText('System overview')).not.toBeInTheDocument();
  expect(screen.getAllByRole('switch')).toHaveLength(1);
  await user.click(enable);
  expect(await screen.findByText('System overview')).toBeVisible();
  expect(screen.getAllByRole('switch')).toHaveLength(1);
  expect(screen.getByRole('switch')).toBeChecked();
  expect(calls.write).toHaveBeenCalledExactlyOnceWith(true);
});

it('shows the generic private notice from the served place, and nothing for an ordinary answer', () => {
  const shown = draw(<AdminReachNotice private of={{ id: 'department', kind: 'org', name: 'Research', redacted: false }} />);
  expect(screen.getByRole('img', { name: 'Private · You see this as an admin of Research' })).toBeVisible();
  shown.rerender(<AdminReachNotice of={null} />);
  expect(screen.queryByRole('img', { name: /You see this as an admin/ })).not.toBeInTheDocument();
});

it('keeps one session control in the top bar and none in the actual Resources navigation', async () => {
  const user = userEvent.setup();
  draw(<><header data-testid="titlebar"><AdminModeControl /></header><header data-testid="resources"><ResourceNav /></header></>);
  await waitFor(() => expect(screen.getAllByRole('switch', { name: ADMIN_MODE_NAME })).toHaveLength(1));
  expect(within(screen.getByTestId('resources')).queryByRole('switch', { name: ADMIN_MODE_NAME })).not.toBeInTheDocument();
  await user.click(within(screen.getByTestId('titlebar')).getByRole('switch', { name: ADMIN_MODE_NAME }));
  await waitFor(() => expect(screen.getByTestId('titlebar').querySelector('button')).toHaveAttribute('data-as-admin', 'true'));
  expect(screen.getAllByRole('switch', { name: ADMIN_MODE_NAME })).toHaveLength(1);
  expect(calls.write).toHaveBeenCalledExactlyOnceWith(true);
  expect(calls.read).toHaveBeenCalled();
});

it('does not offer a self-grant or Open as admin for even an old-style override refusal', () => {
  const error = new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Access denied', detail: 'The page does not give this permission.', status: 403, request_id: 'refusal', missing: [], override: true, requestable: false }, null);
  draw(<NoAccess error={error} />);
  expect(screen.getByRole('heading', { name: 'You don’t have access' })).toBeVisible();
  expect(screen.queryByRole('button', { name: /Open.*admin/ })).not.toBeInTheDocument();
  expect(screen.queryByText(/you add yourself to the page/i)).not.toBeInTheDocument();
});

it('keeps the historical As admin audit fact even when the current session is off', () => {
  const actor = { kind: 'user' as const, user: { id: 'person', username: 'ada', display_name: 'Ada', kind: 'human' as const, avatar: { color: 1, text: 'A' } } };
  const shown = draw(<TraceActor entry={{ actor, as_admin: true }} />);
  expect(screen.getByText('As admin')).toBeVisible();
  shown.rerender(<TraceActor entry={{ actor, as_admin: false }} />);
  expect(screen.queryByText('As admin')).not.toBeInTheDocument();
});

it('shows returned chat sections with no admin opt-in or per-department probes', async () => {
  const user = userEvent.setup();
  draw(<AllChats org={{ org_id: 'root' }} slug="team" search={{}} me="person" />);
  expect(screen.queryByRole('switch')).not.toBeInTheDocument();
  await user.click(screen.getByText(i18n.t('chat.all.others')));
  expect(await screen.findByRole('link', { name: 'A readable conversation' })).toBeVisible();
  expect(calls.chats).toHaveBeenCalledWith('root', { scope: 'others', of: null, participating: false });
});

it('sends the ordinary focus filter to chat listing before pages are requested', async () => {
  const user = userEvent.setup();
  draw(<AllChats org={{ org_id: 'root' }} slug="team" search={{ participating: 'true' }} me="person" />);
  await user.click(screen.getByText(i18n.t('chat.all.nodes')));
  await waitFor(() => expect(calls.chats).toHaveBeenCalledWith('root', { scope: 'nodes', of: null, participating: true }));
});

it('retires mode-only readers before repairing the ordinary view after confirmed mode loss', async () => {
  state = { available: true, as_admin: true };
  cache.setQueryData(['admin-mode'], state);
  cache.setQueryData(systemKey, 'Privileged overview');
  cache.setQueryData(['document', 'ordinary'], 'Ordinary document');
  const adminRead = vi.fn(() => Promise.resolve('Fresh administrative overview'));
  const ordinaryRead = vi.fn(() => Promise.resolve('Current ordinary document'));
  function Privileged() {
    const query = useQuery({ queryKey: systemKey, meta: systemQuery.meta, queryFn: adminRead, staleTime: Infinity });
    return <p>{query.data}</p>;
  }
  function Guard() { const mode = useAdminMode(); return mode.data?.as_admin === true ? <Privileged /> : null; }
  function Ordinary() {
    const query = useQuery({ queryKey: ['document', 'ordinary'], queryFn: ordinaryRead, staleTime: Infinity });
    return <p>{query.data}</p>;
  }
  draw(<><AdminModeControl /><Guard /><Ordinary /></>);
  expect(await screen.findByText('Privileged overview')).toBeVisible();
  expect(adminRead).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('switch', { name: 'As admin: On' }));
  await waitFor(() => expect(screen.getByRole('switch', { name: 'As admin: Off' })).not.toBeChecked());
  expect(screen.queryByText('Privileged overview')).not.toBeInTheDocument();
  expect(adminRead).not.toHaveBeenCalled();
  expect(ordinaryRead).toHaveBeenCalledTimes(1);
  expect(screen.getByText('Current ordinary document')).toBeVisible();
  await userEvent.click(screen.getByRole('switch', { name: 'As admin: Off' }));
  expect(await screen.findByText('Fresh administrative overview')).toBeVisible();
  expect(adminRead).toHaveBeenCalledTimes(1);
});
