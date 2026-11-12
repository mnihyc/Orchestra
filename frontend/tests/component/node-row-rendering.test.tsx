/** A live state change updates its marks without rebuilding unchanged row actions. */
import { act, cleanup, render, screen } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { afterEach, expect, it, vi } from 'vitest';

import { NodeMakeButton, NodeMenuButton } from '@/features/navigation/NodeMenu';
import type * as NodeMenuModule from '@/features/navigation/NodeMenu';
import type * as PlacesModule from '@/features/navigation/places';
import { NodeRow } from '@/features/navigation/NodeTree';
import type { NavItem } from '@/features/navigation/treeRows';
import { i18n } from '@/i18n';
import type { MenuEntry } from '@/ui/Menu';
import { TooltipProvider } from '@/ui/Tooltip';
import { ACME, acmeTree } from '../doubles/fixtures';
import { display } from '../doubles/thread-display';

vi.mock('@/features/navigation/NodeMenu', async (original) => {
  const actual = await original<typeof NodeMenuModule>();
  return { ...actual,
    NodeMenuButton: vi.fn(({ place }: ComponentProps<typeof actual.NodeMenuButton>) => <button>{place.name} actions</button>),
    NodeMakeButton: vi.fn(({ place }: ComponentProps<typeof actual.NodeMakeButton>) => <button>{place.name} create</button>),
  };
});
vi.mock('@/features/navigation/places', async (original) => {
  const actual = await original<typeof PlacesModule>();
  return { ...actual, PlaceLink: ({ place, children, ...props }: ComponentProps<typeof actual.PlaceLink>) => <a href={`#${place.id}`} {...props}>{children}</a> };
});

afterEach(async () => { cleanup(); vi.clearAllMocks(); await i18n.changeLanguage('en'); });

function itemOf(kind: 'thread' | 'project'): Extract<NavItem, { type: 'node' }> {
  const node = acmeTree().find((row) => row.id === (kind === 'thread' ? ACME.mainThread : ACME.shop));
  if (node === undefined) throw new Error('Expected the row fixture.');
  const idle = node.thread === null ? node : { ...node, thread: { ...node.thread, state: 'idle' as const, display: display('idle') } };
  return { type: 'node', node: idle, urgent: null, folder: false, waiting: 0, unread: false, mentions: 0, chat: 'none', chatMentions: 0 };
}

it.each(['thread', 'project'] as const)('updates a %s state without rebuilding its actions, and follows a rename', (kind) => {
  const item = itemOf(kind);
  const view = render(<NodeRow item={item} />, { wrapper: TooltipProvider });
  if (kind === 'thread') expect(screen.getByRole('img', { name: i18n.t('state.idle') })).toBeVisible();
  const menus = vi.mocked(NodeMenuButton).mock.calls.length;
  const creates = vi.mocked(NodeMakeButton).mock.calls.length;
  const node = { ...item.node, thread: item.node.thread === null ? null : { ...item.node.thread, state: 'running' as const, display: display('running') } };
  view.rerender(<NodeRow item={{ ...item, node, urgent: 'running' }} />);
  expect(screen.getByRole('img', { name: i18n.t(kind === 'thread' ? 'state.running' : 'navigation.holds.running') })).toBeVisible();
  expect(NodeMenuButton).toHaveBeenCalledTimes(menus);
  expect(NodeMakeButton).toHaveBeenCalledTimes(creates);

  view.rerender(<NodeRow item={{ ...item, node: { ...node, name: 'Renamed' }, urgent: 'running' }} />);
  expect(NodeMenuButton).toHaveBeenCalledTimes(menus + 1);
  expect(vi.mocked(NodeMenuButton).mock.lastCall?.[0].place.name).toBe('Renamed');
});

it('keeps custom row ends and extra actions current', () => {
  const item = itemOf('project');
  const extra: MenuEntry[] = [{ id: 'show', label: 'Show in tree', onSelect: vi.fn() }];
  const view = render(<NodeRow item={item} end="1m" endTitle="Opened a minute ago" extraEntries={extra} />, { wrapper: TooltipProvider });
  expect(screen.getByText('1m')).toBeVisible();
  expect(vi.mocked(NodeMenuButton).mock.lastCall?.[0].extraEntries).toBe(extra);
  view.rerender(<NodeRow item={item} end="2m" endTitle="Opened two minutes ago" extraEntries={extra} />);
  expect(screen.getByText('2m')).toBeVisible();
  expect(screen.getByTitle('Opened two minutes ago')).toBeVisible();
  expect(vi.mocked(NodeMenuButton).mock.lastCall?.[0].extraEntries).toBe(extra);
});

it('passes the current parent caption to its actions', () => {
  const item = itemOf('thread');
  const view = render(<NodeRow item={{ ...item, parentName: 'Parent' }} />, { wrapper: TooltipProvider });
  expect(vi.mocked(NodeMenuButton).mock.lastCall?.[0].parentName).toBe('Parent');
  view.rerender(<NodeRow item={{ ...item, parentName: 'Renamed parent' }} />);
  expect(vi.mocked(NodeMenuButton).mock.lastCall?.[0].parentName).toBe('Renamed parent');
});

it('refreshes action names when the active language changes', async () => {
  render(<NodeRow item={itemOf('thread')} />, { wrapper: TooltipProvider });
  const menus = vi.mocked(NodeMenuButton).mock.calls.length;
  await act(async () => { await i18n.changeLanguage('zh-CN'); });
  expect(NodeMenuButton).toHaveBeenCalledTimes(menus + 1);
});
