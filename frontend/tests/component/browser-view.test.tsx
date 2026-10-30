/** The Browser view: open browsers before profiles, a watcher told who drives, watching that sends nothing, Drive on a socket of its own, keys given back by Ctrl+Alt+Esc, what a page asks shown to whoever drives, and each way of being off or of finding nothing. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createStore, type StoreApi } from 'zustand/vanilla';

import { BrowsersPanel } from '@/features/browser/Browsers';
import type * as BrowserApi from '@/api/browsers';
import type * as SocketModule from '@/features/browser/socket';
import type { BrowserControlFrame, BrowserSocketState } from '@/features/browser/socket';
import type { Browser, Environment, EnvironmentBrowsers } from '@/generated/api-types';
import { show } from './support';

const made = vi.hoisted(() => ({ connections: [] as { control: boolean; sent: BrowserControlFrame[]; state: StoreApi<BrowserSocketState> }[], list: null as EnvironmentBrowsers | null }));

vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: ReactNode }) => <a href="#">{children}</a> }));
vi.mock('@/realtime/live', () => ({ socket: { subscribe: () => ({ unsubscribe: () => undefined }) } }));
vi.mock('@/api/browsers', async (original) => ({
  ...await original<typeof BrowserApi>(),
  browserKeys: { list: (id: string) => ['test-browsers', id] },
  browsersQuery: (id: string) => ({ queryKey: ['test-browsers', id], queryFn: () => Promise.resolve(made.list) }),
  lookAgain: () => Promise.resolve(made.list),
  quitBrowser: vi.fn(),
  deleteProfile: vi.fn(),
  renameProfile: vi.fn(),
  openBrowser: vi.fn(),
}));
vi.mock('@/features/browser/socket', async (original) => {
  const actual = await original<typeof SocketModule>();
  return {
    ...actual,
    connectBrowser: ({ control }: { control: boolean }) => {
      const state = createStore<BrowserSocketState>(() => ({ phase: 'open', control, reason: null, tabs: [{ id: 'tab-1', title: 'Roadmap', url: 'https://www.notion.so/acme/Roadmap', loading: false, active: true, opener: null }], tab: 'tab-1', viewport: { width: 1280, height: 800 }, watching: [], driving: [], dialog: null, chooser: null, quality: null, refused: null }));
      const connection = { control, sent: [] as BrowserControlFrame[], state };
      made.connections.push(connection);
      return { state, watch: vi.fn(), drawn: vi.fn(), send: (frame: BrowserControlFrame) => connection.sent.push(frame), close: vi.fn() };
    },
  };
});

const owner = { id: 'u1', username: 'ada', display_name: 'Ada Lovelace', kind: 'human', avatar: null } as unknown as Environment['owner'];

function environment(permissions: string[]): Environment {
  return { id: 'env-1', name: 'Main', owner, machine: null, reach: 14, browser: true, access: { permissions }, support: { looked_at: null, browser: { programs: [{ name: 'chromium', path: '/usr/bin/chromium', version: '131' }], window: false, playwright_cli: '0.1.1', sandbox: true }, vnc: { servers: [], window_manager: null } } } as unknown as Environment;
}

function browser(id: string, change: Partial<Browser>): Browser {
  return { id, environment: { id: 'env-1', name: 'Main' } as Browser['environment'], profile: id, origin: 'started', opened_by: null, thread: null, preset: 'headless', display: null, endpoint: 'http://127.0.0.1:41873', program: 'Chromium', tabs: 1, note: null, watching: 1, driving: [], state: 'open', close_reason: null, opened_at: '2026-10-14T10:00:00.000Z', last_used_at: '2026-10-14T10:00:00.000Z', closed_at: null, ...change };
}

function list(change: Partial<EnvironmentBrowsers> = {}): EnvironmentBrowsers {
  return {
    off: null,
    support: { programs: [{ name: 'chromium', path: '/usr/bin/chromium', version: '131' }], window: false, playwright_cli: '0.1.1', sandbox: true },
    presets: [{ name: 'headless', available: true, reason: null }, { name: 'window', available: false, reason: 'No display here.' }],
    profiles: [{ name: 'notion', size_bytes: 1, used_at: null, browser: 'notion' }, { name: 'twitter', size_bytes: 1, used_at: null, browser: null }],
    profiles_dir: null,
    browsers: [browser('notion', { opened_by: owner, tabs: 3 }), browser('agent', { profile: 'thread-9c1f', thread: { id: 't1', kind: 'thread', name: 'Fix login', redacted: false } })],
    limit: { open: 2, max: 4 },
    ...change,
  };
}

function draw(permissions: string[], threadId: string | null = 't1') {
  show(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <BrowsersPanel environment={environment(permissions)} threadId={threadId} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  made.connections = [];
  made.list = list();
  vi.stubGlobal('ResizeObserver', class { observe() { return undefined; } disconnect() { return undefined; } });
});

describe('the list and the one in front', () => {
  it('lists the thread agent\'s browser first, then the others, then the profiles none runs on, and tells a watcher who drives', async () => {
    draw(['browser.view']);
    const listed = await screen.findByRole('list', { name: 'Browsers and profiles' });
    expect(listed.textContent.indexOf('thread-9c1f')).toBeLessThan(listed.textContent.indexOf('notion'));
    expect(screen.getByText('a profile · not open')).toBeInTheDocument();
    expect(screen.getByText(/You can watch. Driving is with whoever manages Main/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Drive' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Open a browser…' })).toBeNull();
  });

  it('sends nothing while watching, drives on a socket of its own, and gives the keys back on Ctrl+Alt+Esc', async () => {
    const user = userEvent.setup();
    draw(['browser.view', 'browser.manage']);
    await screen.findByRole('button', { name: 'Drive' });
    expect(made.connections.at(-1)?.control).toBe(false);
    await user.keyboard('abc');
    expect(made.connections.flatMap((connection) => connection.sent)).toEqual([]);
    await user.click(screen.getByRole('button', { name: 'Drive' }));
    await waitFor(() => expect(made.connections.at(-1)?.control).toBe(true));
    await user.keyboard('h');
    await waitFor(() => expect(made.connections.at(-1)?.sent.some((frame) => frame.op === 'key' && frame.key === 'h' && frame.text === 'h')).toBe(true));
    await user.keyboard('{Control>}{Alt>}{Escape}{/Alt}{/Control}');
    expect(await screen.findByRole('button', { name: 'Drive' })).toBeInTheDocument();
    expect(made.connections.at(-1)?.control).toBe(false);
  });

  it('shows what a page asks to whoever drives, and sends the answer', async () => {
    const user = userEvent.setup();
    draw(['browser.view', 'browser.manage']);
    await user.click(await screen.findByRole('button', { name: 'Drive' }));
    await waitFor(() => expect(made.connections.at(-1)?.control).toBe(true));
    made.connections.at(-1)?.state.setState({ dialog: { tab: 'tab-1', kind: 'confirm', message: 'Leave this page?', default: null } });
    await user.click(await screen.findByRole('button', { name: 'OK' }));
    expect(made.connections.at(-1)?.sent).toContainEqual({ op: 'dialog', tab: 'tab-1', accept: true });
  });
});

describe('what is on and what was found are two things', () => {
  it('says that nothing was found, with what was looked for and Look again', async () => {
    made.list = list({ browsers: [], profiles: [], support: { programs: [], window: false, playwright_cli: null, sandbox: null } });
    draw(['browser.view', 'browser.manage']);
    expect(await screen.findByText('No browser was found in Main')).toBeInTheDocument();
    expect(screen.getByText(/Looked for: Playwright, chromium/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Look again' })).toBeInTheDocument();
  });

  it('says by whom browsers are off', async () => {
    made.list = list({ off: 'environment_switch_off' });
    draw(['browser.view', 'browser.manage']);
    expect(await screen.findByText(/Browsers are switched off for Main/)).toBeInTheDocument();
  });
});
