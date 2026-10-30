/** Inline viewer choices use compact state-bearing names, while permissions and viewer selection stay intact. */
import { act, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';

import { BrowsersSection, DisplaysSection } from '@/features/workbench/TargetSections';
import { useBrowserRequests } from '@/features/browser/store';
import type { Browser, Display } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

// These panels only read the authorized target identities, names and states from the larger replies.
const world = vi.hoisted(() => ({
  permissions: ['browser.view', 'vnc.view'],
  browsers: [] as Pick<Browser, 'id' | 'profile' | 'state'>[],
  displays: [] as Pick<Display, 'id' | 'name' | 'state'>[],
  refresh: new Map<string, () => void>(),
  reveal: vi.fn(), browserRead: vi.fn(), displayRead: vi.fn(),
}));

vi.mock('@/features/files/door', () => ({
  useRouteThread: () => 'thread',
  useThreadDoor: () => ({ kind: 'ready', access: { environment: { id: 'environment' }, thread: { environment_permissions: world.permissions } } }),
}));
vi.mock('@/features/workbench/openView', () => ({ revealView: world.reveal }));
vi.mock('@/api/browsers', () => ({
  browserKeys: { list: (id: string) => ['target-browsers', id] },
  browsersQuery: (id: string) => ({ queryKey: ['target-browsers', id], queryFn: () => { world.browserRead(); return Promise.resolve({ browsers: world.browsers }); } }),
}));
vi.mock('@/api/displays', () => ({
  displayKeys: { list: (id: string) => ['target-displays', id] },
  displaysQuery: (id: string) => ({ queryKey: ['target-displays', id], queryFn: () => { world.displayRead(); return Promise.resolve({ displays: world.displays }); } }),
}));
vi.mock('@/realtime/live', () => ({ socket: { subscribe: (name: string, handlers: { onFrame: (frame: { op: 'sync' }) => void }) => {
  world.refresh.set(name, () => handlers.onFrame({ op: 'sync' }));
  return { unsubscribe: () => { world.refresh.delete(name); } };
} } }));

beforeEach(() => {
  vi.clearAllMocks();
  world.permissions = ['browser.view', 'vnc.view'];
  world.browsers = [{ id: 'browser', profile: 'Work', state: 'opening' }, { id: 'closed-browser', profile: 'Old browser', state: 'closed' }];
  world.displays = [{ id: 'display', name: 'Desktop', state: 'opening' }, { id: 'closed-display', name: 'Old display', state: 'closed' }];
  world.refresh.clear();
  useBrowserRequests.setState({ front: null, display: null, open: null });
});

it.each([['en', 'Starting', 'Open'], ['zh-CN', '正在启动', '已打开']] as const)('shows the current state in each compact label and opens the chosen viewer in %s', async (locale, starting, open) => {
  try {
    await setLocale(locale);
    const user = userEvent.setup();
    show(<><BrowsersSection /><DisplaysSection /></>);
    const browser = await screen.findByRole('button', { name: `Work · ${starting}` });
    const display = await screen.findByRole('button', { name: `Desktop · ${starting}` });
    for (const list of [screen.getByRole('list', { name: i18n.t('browser.list.label') }), screen.getByRole('list', { name: i18n.t('vnc.list.label') })]) {
      expect(within(list).getAllByRole('button')).toHaveLength(1);
      expect(list.querySelector('[data-choice-description], [data-choice-help]')).toBeNull();
    }
    expect(browser).toHaveTextContent(`Work · ${starting}`);
    expect(display).toHaveTextContent(`Desktop · ${starting}`);
    await user.click(browser);
    expect(useBrowserRequests.getState().front).toBe('browser');
    expect(world.reveal).toHaveBeenLastCalledWith('browser');
    display.focus();
    await user.keyboard('{Enter}');
    expect(useBrowserRequests.getState().display).toBe('display');
    expect(world.reveal).toHaveBeenLastCalledWith('vnc');
    act(() => {
      world.browsers = [{ id: 'browser', profile: 'Work', state: 'open' }];
      world.displays = [{ id: 'display', name: 'Desktop', state: 'open' }];
      world.refresh.get('env:environment:browsers')?.();
      world.refresh.get('env:environment:displays')?.();
    });
    expect(await screen.findByRole('button', { name: `Work · ${open}` })).toHaveTextContent(`Work · ${open}`);
    expect(await screen.findByRole('button', { name: `Desktop · ${open}` })).toHaveTextContent(`Desktop · ${open}`);
  } finally { await setLocale('en'); }
});

it('reads neither target list without its environment permission', () => {
  world.permissions = [];
  show(<><BrowsersSection /><DisplaysSection /></>);
  expect(screen.queryByRole('list', { name: i18n.t('browser.list.label') })).not.toBeInTheDocument();
  expect(screen.queryByRole('list', { name: i18n.t('vnc.list.label') })).not.toBeInTheDocument();
  expect(document.querySelector('[data-workbench-target]')).not.toBeInTheDocument();
  expect(world.browserRead).not.toHaveBeenCalled();
  expect(world.displayRead).not.toHaveBeenCalled();
  expect(world.refresh.size).toBe(0);
});
