/** The VNC view: a display drawn by the viewer on a socket the view opens with a ticket, told to send nothing while watching, given a socket to work in only after "Work in it", the copied sentence on a display Orch did not open, and Close in its own words for each kind. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { DisplaysPanel } from '@/features/vnc/Displays';
import type { RfbConstructor, RfbLike } from '@/features/vnc/Viewer';
import type { Display, Environment, EnvironmentDisplays } from '@/generated/api-types';
import { show } from './support';

const made = vi.hoisted(() => ({ tickets: [] as boolean[], viewers: [] as { viewOnly: boolean; channel: unknown }[], sockets: [] as { url: string; protocol: string }[], list: null as EnvironmentDisplays | null }));

vi.mock('@/realtime/live', () => ({ socket: { subscribe: () => ({ unsubscribe: () => undefined }) } }));
vi.mock('@/api/displays', () => ({
  displayKeys: { list: (id: string) => ['test-displays', id] },
  displaysQuery: (id: string) => ({ queryKey: ['test-displays', id], queryFn: () => Promise.resolve(made.list) }),
  lookAgainForDisplays: () => Promise.resolve(made.list),
  openDisplay: vi.fn(),
  closeDisplay: vi.fn(() => Promise.resolve({})),
  displayTicket: (_id: string, control: boolean) => {
    made.tickets.push(control);
    return Promise.resolve({ ticket: `ticket-${made.tickets.length}`, ticket_expires_at: '' });
  },
}));

class FakeSocket extends EventTarget {
  binaryType = 'blob';
  readyState = 1;
  constructor(readonly url: string, readonly protocol: string) {
    super();
    made.sockets.push({ url, protocol });
  }
  close() { this.readyState = 3; }
  send() { return undefined; }
}

class FakeRfb extends EventTarget implements RfbLike {
  viewOnly = false;
  focusOnClick = true;
  scaleViewport = false;
  resizeSession = false;
  qualityLevel = 6;
  constructor(_target: HTMLElement, channel: WebSocket) {
    super();
    const entry = { viewOnly: false, channel: channel as unknown };
    made.viewers.push(entry);
    queueMicrotask(() => {
      entry.viewOnly = this.viewOnly;
      this.dispatchEvent(new Event('connect'));
    });
  }
  disconnect() { return undefined; }
  sendCtrlAltDel() { return undefined; }
  focus() { return undefined; }
  blur() { return undefined; }
  clipboardPasteFrom() { return undefined; }
}

const load = (): Promise<RfbConstructor> => Promise.resolve(FakeRfb);

const owner = { id: 'u1', username: 'ada', display_name: 'Ada Lovelace', kind: 'human', avatar: null } as unknown as Environment['owner'];
const environment = (permissions: string[]) => ({ id: 'env-1', name: 'Main', owner, machine: null, vnc: true, access: { permissions } }) as unknown as Environment;

function display(id: string, change: Partial<Display>): Display {
  return { id, environment: { id: 'env-1', name: 'Main' } as Display['environment'], name: id, origin: 'started', opened_by: owner, thread: null, endpoint: null, x_display: ':1', geometry: '1280x800', watching: 1, driving: [], note: null, browsers: [], state: 'open', close_reason: null, opened_at: '', last_used_at: '', closed_at: null, ...change };
}

function draw(permissions: string[]) {
  show(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <DisplaysPanel environment={environment(permissions)} threadId={null} load={load} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  made.tickets = [];
  made.viewers = [];
  made.sockets = [];
  made.list = { off: null, support: { servers: [{ name: 'Xvnc', path: '/usr/bin/Xvnc', version: '1.14.1' }], window_manager: 'openbox' }, displays: [display('Desktop', {}), display('emulator', { origin: 'announced', x_display: null, opened_by: null, note: 'The Android emulator' })], limit: { open: 1, max: 4 } };
  vi.stubGlobal('WebSocket', FakeSocket);
});

describe('one display in front', () => {
  it('connects the viewer on a socket to watch, and gives it one to work in only after Work in it', async () => {
    const user = userEvent.setup();
    draw(['vnc.view', 'vnc.manage']);
    await waitFor(() => expect(made.viewers).toHaveLength(1));
    await waitFor(() => expect(made.viewers[0]?.viewOnly).toBe(true));
    expect(made.tickets).toEqual([false]);
    expect(made.sockets[0]?.protocol).toBe('orch.display.v1');
    expect(screen.getByText('DISPLAY=:1')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Work in it' }));
    await waitFor(() => expect(made.viewers).toHaveLength(2));
    await waitFor(() => expect(made.viewers[1]?.viewOnly).toBe(false));
    expect(made.tickets).toEqual([false, true]);
    expect(screen.getByRole('button', { name: 'Stop working in it' })).toBeInTheDocument();
  });

  it('says what is copied may reach every watcher only on a display Orch did not open', async () => {
    const user = userEvent.setup();
    draw(['vnc.view']);
    await screen.findByRole('button', { name: /Desktop/ });
    expect(screen.queryByText('What is copied in this display may reach everybody who watches it.')).toBeNull();
    await user.click(screen.getByRole('button', { name: /emulator/ }));
    expect(await screen.findByText('What is copied in this display may reach everybody who watches it.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Work in it' })).toBeNull();
    expect(screen.getByText(/You can watch. Working in it is with whoever manages Main/)).toBeInTheDocument();
  });
});

describe('closing says what happens to each kind', () => {
  it('ends what shows on one Orch opened, and only stops showing one an agent announced', async () => {
    const user = userEvent.setup();
    draw(['vnc.view', 'vnc.manage']);
    const list = await screen.findByRole('list', { name: 'Displays' });
    const menus = await waitFor(() => {
      const found = list.querySelectorAll('button[aria-label="More for this display"]');
      expect(found).toHaveLength(2);
      return found;
    });
    await user.click(menus[0]!);
    await user.click(await screen.findByRole('menuitem', { name: 'Close…' }));
    expect(await screen.findByText('What shows on it ends with it.')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Cancel' }));
    await user.click(menus[1]!);
    await user.click(await screen.findByRole('menuitem', { name: 'Close…' }));
    expect(await screen.findByText('It is no longer shown here. Its program runs on.')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Stop showing it' })).toBeInTheDocument();
  });

  it('says that nothing was found to serve a display', async () => {
    made.list = { off: null, support: { servers: [], window_manager: null }, displays: [], limit: { open: 0, max: 4 } };
    draw(['vnc.view', 'vnc.manage']);
    expect(await screen.findByText('No display server was found in Main')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Look again' })).toBeInTheDocument();
  });
});
