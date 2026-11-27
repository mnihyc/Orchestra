/** A row of the tree makes its menus only when one opens, as a place's and a document's do: a shortcut's row is drawn with nothing read or kept for its acts, and a chat's row reads whether its chat is starred only once its menu is open. */
import type * as RouterModule from '@tanstack/react-router';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';

import { useIsStarred } from '@/features/navigation/favorites';
import { NodeTree } from '@/features/navigation/NodeTree';
import { ShortcutContextMenu, ShortcutMenuButton } from '@/features/navigation/shortcuts';
import type { Shortcut, TreeNode } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { TooltipProvider } from '@/ui/Tooltip';
import { show } from './support';

vi.mock('@/features/navigation/favorites', async (loadOriginal) => ({ ...await loadOriginal<object>(), useIsStarred: vi.fn(() => false) }));
vi.mock('@tanstack/react-router', async (loadRouter) => ({ ...await loadRouter<typeof RouterModule>(), Link: ({ children }: { children: ReactNode }) => <a href="#place">{children}</a>, useRouter: () => ({ invalidate: vi.fn() }) }));
vi.mock('@/features/navigation/NodeMenu', async (loadOriginal) => ({ ...await loadOriginal<object>(), NodeContextMenu: ({ children }: { children: ReactNode }) => children, NodeMakeButton: () => null, NodeMenuButton: () => null, useIsPending: () => false }));
vi.mock('@/features/chat/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), isPlainClick: () => true, useOpenPath: () => vi.fn() }));

const shortcut: Shortcut = {
  id: 'kept',
  node_id: 'web',
  target: { kind: 'document', id: 'plan' },
  position: '1',
  state: 'open',
  item: { name: 'Launch plan', path: [{ id: 'acme', kind: 'org', name: 'Acme', redacted: false }], org_id: 'acme', archived: false, color: null },
  org_id: null,
  view: null,
};

const node = (id: string, parent_id: string | null, seq: number): TreeNode => ({
  id, parent_id, kind: parent_id === null ? 'org' : 'project', name: id, address: null, color: null,
  archived: false, deleted: false, restricted: false, tree_seq: 1, topic: null, thread: null,
  chat: { id: `chat-${id}`, seq }, docs: 0, docs_seq: 0, shortcuts: 0, participating: true, admin_access: null,
});

beforeEach(() => { vi.mocked(useIsStarred).mockClear(); });

it('draws a shortcut row with both its menus and nothing made for them until one opens, without the page\'s reads and acts', () => {
  // No query client is given: a menu that made its acts with the row would ask for one.
  render(
    <ShortcutContextMenu shortcut={shortcut}>
      <div><ShortcutMenuButton shortcut={shortcut} /></div>
    </ShortcutContextMenu>,
    { wrapper: TooltipProvider },
  );
  expect(screen.getByRole('button', { name: i18n.t('places.menu.more', { name: 'Launch plan' }) })).toBeVisible();
  expect(screen.queryByRole('menuitem')).not.toBeInTheDocument();
});

it('reads whether a chat in the tree is starred only once its row\'s menu opens, and offers its star and shortcut there', async () => {
  const user = userEvent.setup();
  const tree = { nodes: new Map([['root', node('root', null, 0)], ['web', node('web', 'root', 3)]]), lastSeq: 1, loaded: true, error: null };
  show(<NodeTree tree={tree} rootId="root" view={{ chatRows: true }} expanded={new Set(['web'])} onExpandedChange={vi.fn()} currentId={null} locale="en" me="ada" />);
  const of = i18n.t('navigation.chat.of', { name: 'web' });
  const more = screen.getByRole('button', { name: i18n.t('places.menu.more', { name: of }) });
  expect(useIsStarred).not.toHaveBeenCalled();

  await user.click(more);
  const menu = screen.getByRole('menu', { name: i18n.t('places.menu.of', { name: of }) });
  expect(within(menu).getByRole('menuitem', { name: i18n.t('places.star.star') })).toBeVisible();
  expect(within(menu).getByRole('menuitem', { name: i18n.t('shortcuts.add') })).toBeVisible();
  expect(useIsStarred).toHaveBeenCalledWith('chat-web');
});
