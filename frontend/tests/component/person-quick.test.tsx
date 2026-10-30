/** Person cards separate quick actions from the kept profile and send only on a choice. */
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClientProvider, useQuery } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import type * as PersonActions from '@/api/personActions';
import { personActionsQuery } from '@/api/personActions';
import { LiveQueryClient } from '@/api/query';
import { client as apiClient } from '@/api/client';
import type { UserRef } from '@/generated/api-types';
import { PersonThingProvider } from '@/features/people/personContext';
import { MentionPrompt } from '@/features/people/MentionPrompt';
import { UserChip } from '@/ui/UserChip';
import { show } from './support';

const calls = vi.hoisted(() => {
  const state: { can_view: boolean | null; can_notify: boolean; gives: ('full_view' | 'full_interact')[]; note_chars: number } = { can_view: true, can_notify: true, gives: [], note_chars: 200 };
  return { notify: vi.fn<typeof PersonActions.notifyPerson>(), grant: vi.fn<typeof PersonActions.grantPerson>(), read: vi.fn<() => void>(), profileRead: vi.fn<() => void>(), profileSelf: false, state };
});
vi.mock('@/api/personActions', () => ({
  personActionsQuery: (thing: { kind: string; id: string }, userId: string) => ({ queryKey: ['person-actions', thing.kind, thing.id, userId], queryFn: () => { calls.read(); return Promise.resolve(calls.state); }, retry: false }),
  notifyPerson: calls.notify,
  grantPerson: calls.grant,
}));
vi.mock('@/api/profile', () => ({ profileQuery: (username: string) => ({ queryKey: ['profile', username], queryFn: () => { calls.profileRead(); return Promise.resolve({ is_me: calls.profileSelf, can_message: false }); } }) }));
vi.mock('@/features/profile/CardDetails', () => ({ CardDetails: () => <div>Full profile</div> }));

const user: UserRef = { id: 'person', username: 'rui', display_name: 'Rui', kind: 'human', avatar: { text: 'R', color: 1 } };
const thing = { kind: 'document', id: 'doc', anchor: { block: 'block' } } as const;
afterEach(() => { vi.unstubAllGlobals(); calls.notify.mockReset(); calls.grant.mockReset(); calls.read.mockClear(); calls.profileRead.mockClear(); calls.profileSelf = false; });

it('focus offers a notice while Enter keeps the profile open', async () => {
  calls.state = { can_view: true, can_notify: true, gives: [], note_chars: 200 };
  calls.notify.mockResolvedValue(undefined);
  const input = userEvent.setup();
  show(<PersonThingProvider thing={thing}><UserChip user={user} /></PersonThingProvider>);
  await input.tab();
  expect(screen.getByRole('button', { name: 'Rui' })).toHaveFocus();
  expect(await screen.findByText('Can view')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Notify' })).toBeVisible();
  expect(calls.notify).not.toHaveBeenCalled();
  await input.keyboard('{Enter}');
  const profile = await screen.findByRole('dialog', { name: 'Rui' });
  expect(within(profile).getByText('Full profile')).toBeVisible();
});

it('keyboard users can enter a note and explicitly send it', async () => {
  calls.state = { can_view: true, can_notify: true, gives: [], note_chars: 200 };
  calls.notify.mockResolvedValue(undefined);
  const input = userEvent.setup();
  show(<PersonThingProvider thing={thing}><UserChip user={user} /></PersonThingProvider>);
  await input.tab();
  await screen.findByRole('button', { name: 'Notify' });
  await input.tab();
  expect(screen.getByRole('button', { name: 'Notify' })).toHaveFocus();
  await input.keyboard('{Enter}');
  expect(screen.getByRole('textbox', { name: 'Note (optional)' })).toHaveFocus();
  await input.type(screen.getByRole('textbox', { name: 'Note (optional)' }), 'Please look');
  await input.keyboard('{Enter}');
  await waitFor(() => expect(calls.notify).toHaveBeenCalledWith(thing, user.id, 'Please look', expect.any(String)));
  expect(await screen.findByText('Notification sent')).toBeVisible();
  expect(screen.getByRole('status')).toHaveFocus();
});

it('a mention without access changes nothing until one of its offered acts is chosen', async () => {
  calls.grant.mockResolvedValue(undefined);
  const onClose = vi.fn();
  const input = userEvent.setup();
  show(<MentionPrompt user={user} thing={thing} canView={false} onClose={onClose}><span>Rui</span></MentionPrompt>);
  expect(await screen.findByText("Rui can't open this document")).toBeVisible();
  expect(calls.grant).not.toHaveBeenCalled();
  await input.click(screen.getByRole('button', { name: 'Give access and notify' }));
  await waitFor(() => expect(calls.grant).toHaveBeenCalledWith(thing, user.id, 'full_view', true, expect.any(String)));
  expect(onClose).toHaveBeenCalled();
});

it('keeps the mention prompt open while its chosen notification is pending', async () => {
  calls.notify.mockImplementation(() => new Promise<void>(() => undefined));
  const onClose = vi.fn();
  const input = userEvent.setup();
  show(<MentionPrompt user={user} thing={thing} canView onClose={onClose}><span>Rui</span></MentionPrompt>);
  await input.click(await screen.findByRole('button', { name: 'Notify Rui' }));
  expect(await screen.findByRole('button', { name: 'Sending…' })).toBeVisible();
  expect(screen.getByRole('button', { name: 'Dismiss' })).toBeDisabled();
  expect(onClose).not.toHaveBeenCalled();
});

it('a touch opens the profile with its quick actions above it', async () => {
  vi.stubGlobal('matchMedia', () => ({ matches: true, media: '(hover: none)', addEventListener: vi.fn(), removeEventListener: vi.fn() }));
  calls.state = { can_view: true, can_notify: true, gives: [], note_chars: 200 };
  const input = userEvent.setup();
  show(<PersonThingProvider thing={thing}><UserChip user={user} /></PersonThingProvider>);
  await input.click(screen.getByRole('button', { name: 'Rui' }));
  const profile = await screen.findByRole('dialog', { name: 'Rui' });
  expect(within(profile).getByText('Full profile')).toBeVisible();
  expect(await within(profile).findByRole('button', { name: 'Notify' })).toBeVisible();
});

it('does not disclose access when the server withholds the holder state', async () => {
  calls.state = { can_view: null, can_notify: false, gives: [], note_chars: 200 };
  const input = userEvent.setup();
  show(<PersonThingProvider thing={thing}><UserChip user={user} /></PersonThingProvider>);
  await input.tab();
  await screen.findByText('@rui');
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Notify' })).not.toBeInTheDocument());
  expect(screen.queryByText('Can view')).not.toBeInTheDocument();
  expect(screen.queryByText('No access')).not.toBeInTheDocument();
});

it('uses the current note setting for whole Unicode characters', async () => {
  calls.state = { can_view: true, can_notify: true, gives: [], note_chars: 3 };
  const input = userEvent.setup();
  show(<PersonThingProvider thing={thing}><UserChip user={user} /></PersonThingProvider>);
  await input.tab();
  await input.click(await screen.findByRole('button', { name: 'Notify' }));
  const note = screen.getByRole('textbox', { name: 'Note (optional)' });
  fireEvent.change(note, { target: { value: '好😀xy' } });
  expect(note).toHaveValue('好😀x');
});

it('sends without a note when notes are switched off', async () => {
  calls.state = { can_view: true, can_notify: true, gives: [], note_chars: 0 };
  calls.notify.mockResolvedValue(undefined);
  const input = userEvent.setup();
  show(<PersonThingProvider thing={thing}><UserChip user={user} /></PersonThingProvider>);
  await input.tab();
  await input.click(await screen.findByRole('button', { name: 'Notify' }));
  expect(screen.getByText('Notes off')).toBeVisible();
  expect(screen.queryByRole('textbox', { name: 'Note (optional)' })).not.toBeInTheDocument();
  await input.click(screen.getByRole('button', { name: 'Send' }));
  await waitFor(() => expect(calls.notify).toHaveBeenCalledWith(thing, user.id, '', expect.any(String)));
});

it('offers the chat participation level returned by the server', async () => {
  calls.state = { can_view: false, can_notify: false, gives: ['full_interact'], note_chars: 200 };
  calls.grant.mockResolvedValue(undefined);
  const chat = { kind: 'chat', id: 'chat', anchor: { message: 'message' } } as const;
  const input = userEvent.setup();
  show(<PersonThingProvider thing={chat}><UserChip user={user} /></PersonThingProvider>);
  await input.tab();
  await input.click(await screen.findByRole('button', { name: /Give access/ }));
  expect(screen.getByRole('button', { name: 'Can edit' })).toHaveAttribute('aria-pressed', 'true');
  await input.click(screen.getByRole('button', { name: /^Give access$/ }));
  await waitFor(() => expect(calls.grant).toHaveBeenCalledWith(chat, user.id, 'full_interact', false, expect.any(String)));
});

it('retries an unchanged note with one key and gives a changed note a new key', async () => {
  calls.state = { can_view: true, can_notify: true, gives: [], note_chars: 200 };
  calls.notify.mockRejectedValue(new Error('offline'));
  const input = userEvent.setup();
  show(<PersonThingProvider thing={thing}><UserChip user={user} /></PersonThingProvider>);
  await input.tab(); await input.click(await screen.findByRole('button', { name: 'Notify' }));
  const note = screen.getByRole('textbox', { name: 'Note (optional)' });
  await input.type(note, 'First');
  const send = screen.getByRole('button', { name: 'Send' });
  await input.click(send); await waitFor(() => expect(calls.notify).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(send).not.toHaveAttribute('aria-busy', 'true'));
  await input.click(send); await waitFor(() => expect(calls.notify).toHaveBeenCalledTimes(2));
  expect(calls.notify.mock.calls[0]?.[3]).toMatch(/^[0-9a-f-]{36}$/);
  expect(calls.notify.mock.calls[1]?.[3]).toBe(calls.notify.mock.calls[0]?.[3]);
  await waitFor(() => expect(send).not.toHaveAttribute('aria-busy', 'true'));
  await input.type(note, ' again'); await input.click(send);
  await waitFor(() => expect(calls.notify).toHaveBeenCalledTimes(3));
  expect(calls.notify.mock.calls[2]?.[2]).toBe('First again');
  expect(calls.notify.mock.calls[2]?.[3]).not.toBe(calls.notify.mock.calls[1]?.[3]);
});

it('keeps the note fixed while its notice is pending', async () => {
  calls.state = { can_view: true, can_notify: true, gives: [], note_chars: 200 };
  calls.notify.mockImplementation(() => new Promise<void>(() => undefined));
  const input = userEvent.setup();
  show(<PersonThingProvider thing={thing}><UserChip user={user} /></PersonThingProvider>);
  await input.tab(); await input.click(await screen.findByRole('button', { name: 'Notify' }));
  const note = screen.getByRole('textbox', { name: 'Note (optional)' });
  await input.type(note, 'First'); await input.click(screen.getByRole('button', { name: 'Send' }));
  await waitFor(() => expect(note).toBeDisabled());
  expect(note.closest('form')).toHaveFocus();
});

it('retries the same grant with one key and gives changed choices a new key', async () => {
  calls.state = { can_view: false, can_notify: false, gives: ['full_view', 'full_interact'], note_chars: 200 };
  calls.grant.mockRejectedValue(new Error('offline'));
  const input = userEvent.setup();
  show(<PersonThingProvider thing={thing}><UserChip user={user} /></PersonThingProvider>);
  await input.tab(); await input.click(await screen.findByRole('button', { name: /Give access/ }));
  const give = screen.getByRole('button', { name: /^Give access$/ });
  await input.click(give); await waitFor(() => expect(calls.grant).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(give).not.toHaveAttribute('aria-busy', 'true'));
  await input.click(give); await waitFor(() => expect(calls.grant).toHaveBeenCalledTimes(2));
  expect(calls.grant.mock.calls[0]?.[4]).toMatch(/^[0-9a-f-]{36}$/);
  expect(calls.grant.mock.calls[1]?.[4]).toBe(calls.grant.mock.calls[0]?.[4]);
  await waitFor(() => expect(give).not.toHaveAttribute('aria-busy', 'true'));
  await input.click(screen.getByRole('checkbox', { name: 'Notify them' }));
  await input.click(screen.getByRole('button', { name: 'Can edit' }));
  await input.click(give); await waitFor(() => expect(calls.grant).toHaveBeenCalledTimes(3));
  expect(calls.grant.mock.calls[2]?.[2]).toBe('full_interact');
  expect(calls.grant.mock.calls[2]?.[3]).toBe(true);
  expect(calls.grant.mock.calls[2]?.[4]).not.toBe(calls.grant.mock.calls[1]?.[4]);
});

it('keeps access choices fixed while the grant is pending', async () => {
  calls.state = { can_view: false, can_notify: false, gives: ['full_view', 'full_interact'], note_chars: 200 };
  calls.grant.mockImplementation(() => new Promise<void>(() => undefined));
  const input = userEvent.setup();
  show(<PersonThingProvider thing={thing}><UserChip user={user} /></PersonThingProvider>);
  await input.tab(); await input.click(await screen.findByRole('button', { name: /Give access/ }));
  await input.click(screen.getByRole('button', { name: /^Give access$/ }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Can edit' })).toBeDisabled());
  expect(screen.getByRole('button', { name: 'Can view' })).toBeDisabled();
  expect(screen.getByRole('checkbox', { name: 'Notify them' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Can edit' }).closest('form')).toHaveFocus();
});

it('a mention prompt gives a changed access act a new key', async () => {
  calls.grant.mockRejectedValue(new Error('offline'));
  const input = userEvent.setup();
  show(<MentionPrompt user={user} thing={thing} canView={false} onClose={vi.fn()}><span>Rui</span></MentionPrompt>);
  const view = await screen.findByRole('button', { name: 'Give view access' });
  await input.click(view); await waitFor(() => expect(calls.grant).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(view).not.toHaveAttribute('aria-busy', 'true'));
  await input.click(view); await waitFor(() => expect(calls.grant).toHaveBeenCalledTimes(2));
  expect(calls.grant.mock.calls[0]?.[4]).toMatch(/^[0-9a-f-]{36}$/);
  expect(calls.grant.mock.calls[1]?.[4]).toBe(calls.grant.mock.calls[0]?.[4]);
  await waitFor(() => expect(view).not.toHaveAttribute('aria-busy', 'true'));
  await input.click(screen.getByRole('button', { name: 'Give access and notify' }));
  await waitFor(() => expect(calls.grant).toHaveBeenCalledTimes(3));
  expect(calls.grant.mock.calls[2]?.[3]).toBe(true);
  expect(calls.grant.mock.calls[2]?.[4]).not.toBe(calls.grant.mock.calls[1]?.[4]);
});

it.each(['card', 'prompt'] as const)('refreshes one granted person from the %s with the production client among 1000 cached contexts', async (surface) => {
  calls.state = { can_view: false, can_notify: false, gives: ['full_view'], note_chars: 200 };
  calls.grant.mockImplementation(() => {
    calls.state = { can_view: true, can_notify: true, gives: [], note_chars: 200 };
    return Promise.resolve();
  });
  const cache = new LiveQueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } });
  for (let index = 0; index < 1000; index += 1) cache.setQueryData(['person-actions', thing.kind, thing.id, `other-${index}`], calls.state);
  function Reader() {
    const read = useQuery(personActionsQuery(thing, user.id));
    return <span>{read.isPending ? 'Checking reader' : read.data?.can_view === true ? 'Reader ready' : 'Reader waiting'}</span>;
  }
  const input = userEvent.setup();
  const view = show(<QueryClientProvider client={cache}><Reader />{surface === 'card'
    ? <PersonThingProvider thing={thing}><UserChip user={user} /></PersonThingProvider>
    : <MentionPrompt user={user} thing={thing} canView={false} onClose={vi.fn()}><span>Rui</span></MentionPrompt>}
  </QueryClientProvider>);
  try {
    await screen.findByText('Reader waiting');
    if (surface === 'card') {
      await input.tab();
      await input.click(await screen.findByRole('button', { name: /Give access/ }));
    }
    const before = calls.read.mock.calls.length;
    await input.click(screen.getByRole('button', { name: surface === 'card' ? /^Give access$/ : 'Give view access' }));
    expect(await screen.findByText('Reader ready')).toBeVisible();
    expect(calls.read).toHaveBeenCalledTimes(before + 1);
    const others = cache.getQueryCache().findAll({ queryKey: ['person-actions'] }).filter((query) => query.queryKey[3] !== user.id);
    expect(others).toHaveLength(1000);
    expect(others.every((query) => !query.state.isInvalidated)).toBe(true);
  } finally { view.unmount(); cache.clear(); }
});


it.each(['session', 'profile'] as const)('keeps own identity and profile without acts, identified by %s', async (source) => {
  const previous = apiClient.session.getState();
  apiClient.session.setState({ status: source === 'session' ? 'signed_in' : 'unknown', userId: source === 'session' ? user.id : null });
  calls.profileSelf = source === 'profile';
  calls.state = { can_view: true, can_notify: true, gives: ['full_view'], note_chars: 200 };
  const cache = new LiveQueryClient({ defaultOptions: { queries: { retry: false } } });
  const input = userEvent.setup();
  const view = show(<QueryClientProvider client={cache}><PersonThingProvider thing={thing}><UserChip user={user} /></PersonThingProvider></QueryClientProvider>);
  try {
    await input.tab();
    expect(await screen.findByText('@rui')).toBeVisible();
    if (source === 'profile') await waitFor(() => expect(cache.getQueryData<{ is_me: boolean }>(['profile', user.username])?.is_me).toBe(true));
    else expect(calls.profileRead).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: 'Notify' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Give access/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Message' })).not.toBeInTheDocument();
    expect(calls.read).not.toHaveBeenCalled();
    await input.keyboard('{Enter}');
    expect(within(await screen.findByRole('dialog', { name: 'Rui' })).getByText('Full profile')).toBeVisible();
    expect(calls.notify).not.toHaveBeenCalled();
    expect(calls.grant).not.toHaveBeenCalled();
  } finally { view.unmount(); cache.clear(); apiClient.session.setState(previous); }
});

it('keeps a self mention without opening a notification prompt', () => {
  const previous = apiClient.session.getState();
  apiClient.session.setState({ status: 'signed_in', userId: user.id });
  const close = vi.fn();
  const view = show(<MentionPrompt user={user} thing={thing} canView onClose={close}><span>Own mention</span></MentionPrompt>);
  try {
    expect(screen.getByText('Own mention')).toBeVisible();
    expect(screen.queryByRole('dialog', { name: 'Notify Rui' })).not.toBeInTheDocument();
    expect(close).toHaveBeenCalledTimes(1);
    expect(calls.notify).not.toHaveBeenCalled();
    expect(calls.grant).not.toHaveBeenCalled();
  } finally { view.unmount(); apiClient.session.setState(previous); }
});
