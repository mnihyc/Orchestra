/** The server's native-only fact has one form in real resource rows, the tree and a page header. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Box } from 'lucide-react';
import type { ReactNode } from 'react';
import { expect, it, vi } from 'vitest';
import { NodeTree } from '@/features/navigation/NodeTree';
import { ResourceListFrame as ResourceList } from '@/features/resources/ResourceList';
import { ResourceOwner } from '@/features/resources/ResourceOwner';
import { ProviderUsageCard } from '@/features/resources/providers/ProviderUsageCard';
import type { AdminAccess, NodeRef, TreeNode, UserRef } from '@/generated/api-types';
import { PageHeader } from '@/shell/Page';
import { AdminActNotice, adminSpec, isAdminAct } from '@/ui/AdminAct';
import { AsAdminMark } from '@/ui/AsAdminMark';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { show } from './support';
import { planProvider } from '../unit/provider-plan-fixtures';

vi.mock('@/features/navigation/placeLabels', () => ({ usePlaceLabel: () => (place: NodeRef) => place.redacted ? null : place.name }));
vi.mock('@/features/resources/trail', () => ({ rememberList: vi.fn(), useResourceHeading: vi.fn() }));
vi.mock('@/features/resources/ResourceNav', () => ({ ResourceNav: () => null }));
vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: ReactNode }) => <a href="#place">{children}</a>, useRouter: () => ({ invalidate: vi.fn() }) }));
vi.mock('@/features/navigation/NodeMenu', () => ({ NodeContextMenu: ({ children }: { children: ReactNode }) => children, NodeMakeButton: () => null, NodeMenuButton: () => null, useIsPending: () => false }));
vi.mock('@/features/navigation/DocMenu', () => ({ DocContextMenu: ({ children }: { children: ReactNode }) => children, DocMenuButton: () => null }));
vi.mock('@/features/chat/open', () => ({ isPlainClick: () => true, useOpenPath: () => vi.fn() }));

const place: NodeRef = { id: 'department', kind: 'org', name: 'Research', redacted: false };
const reach = { place, private: false };
const owner: UserRef = { id: 'dora', display_name: 'Dora', username: 'dora', kind: 'human', avatar: { text: 'D', color: 2 } };
interface Row { id: string; name: string; owner: UserRef | null; home?: NodeRef | null; access: { permissions: string[]; admin_access: AdminAccess | null; admin_only: string[]; participating: boolean } }
const rows: Row[] = [
  { id: 'mine', name: 'My environment', owner, access: { permissions: [], admin_access: null, admin_only: [], participating: true } },
  { id: 'other', name: 'Their environment', owner, access: { permissions: [], admin_access: reach, admin_only: ['environment.manage'], participating: false } },
];
function List() {
  return <ResourceList kind="environments" search={{}} title="Environments" icon={Box} lead="Work environments" label="Environments"
    load={{ isPending: false, isError: false, error: null, isRefetching: false, refetch: vi.fn() }} all={rows} rows={rows}
    columns={[{ id: 'name', header: 'Name', cell: (row) => <a href={`#${row.id}`}>{row.name}</a> }]} rowKey={(row) => row.id}
    text="" onText={vi.fn()} searchLabel="Find environments" searchPlaceholder="Find environments"
    empty={{ icon: <Box />, title: 'No environments' }} noMatch="No matches" />;
}

it('marks only the native-only served resource row, preserves order and shows its owner on a narrow table', () => {
  show(<List />);
  const table = screen.getByRole('table', { name: 'Environments' });
  const body = within(table).getAllByRole('row').slice(1);
  expect(body).toHaveLength(2);
  expect(within(body[0]!).getByRole('link', { name: 'My environment' })).toBeVisible();
  expect(body[0]!.querySelector('[data-as-admin-mark]')).toBeNull();
  expect(within(body[1]!).getByRole('img', { name: 'You see this as an admin of Research' })).toBeVisible();
  for (const row of body) expect(row.querySelector('[data-resource-owner]')).toHaveTextContent('Dora');
  // jsdom has no table width: the actual Table uses its narrow first-column fallback.
  expect(within(table).queryByRole('columnheader', { name: 'Owner' })).not.toBeInTheDocument();
});

it('uses the same named shield alone in the actual navigation tree, without a mark on ordinary rows', () => {
  const node = (id: string, parent_id: string | null, admin_access: AdminAccess | null): TreeNode => ({
    id, parent_id, kind: parent_id === null ? 'org' : 'project', name: id, address: null, color: null,
    archived: false, deleted: false, restricted: false, tree_seq: 1, topic: null, thread: null,
    chat: { id: `chat-${id}`, seq: 0 }, docs: 0, participating: admin_access === null, admin_access,
  });
  const tree = { nodes: new Map([['root', node('root', null, null)], ['mine', node('mine', 'root', null)], ['other', node('other', 'root', reach)]]), lastSeq: 1, loaded: true, error: null };
  const shown = show(<NodeTree tree={tree} rootId="root" view={{}} expanded={new Set()} onExpandedChange={vi.fn()} currentId={null} locale="en" me="ada" />);
  const mark = screen.getByRole('img', { name: 'You see this as an admin of Research' });
  expect(mark.closest('[role="treeitem"]')).not.toBeNull();
  expect(mark).not.toHaveTextContent('As admin');
  expect(shown.container.querySelectorAll('[data-as-admin-mark]')).toHaveLength(1);
});

it('keeps a private page’s explanation in its header, redacts unknown places and clears it when the marker is absent', () => {
  const shown = show(<PageHeader title="Their environment" mark={<Box />} adminAccess={reach} private />);
  const mark = screen.getByRole('img', { name: 'Private · You see this as an admin of Research' });
  expect(mark.closest('header')).not.toBeNull();
  shown.rerender(<PageHeader title="Their environment" mark={<Box />} />);
  expect(shown.container.querySelector('[data-as-admin-mark]')).toBeNull();
  shown.rerender(<AsAdminMark of={{ ...place, name: null, redacted: true }} />);
  expect(screen.getByRole('img', { name: 'You see this as an admin of a place you cannot see' })).toBeVisible();
});

it('uses the returned resource privacy fact without treating every native-only resource as private', () => {
  const privateReach = { ...reach, private: true };
  const shown = show(<PageHeader title="Their environment" mark={<Box />} adminAccess={reach} />);
  expect(screen.getByRole('img', { name: 'You see this as an admin of Research' })).toBeVisible();
  expect(screen.queryByRole('img', { name: /^Private/ })).not.toBeInTheDocument();
  shown.rerender(<PageHeader title="Their environment" mark={<Box />} adminAccess={privateReach} />);
  expect(screen.getByRole('img', { name: 'Private · You see this as an admin of Research' })).toBeVisible();
});

it('names person, place and system owners rather than an empty owner cell', () => {
  const shown = show(<ResourceOwner owner={owner} />);
  expect(shown.container).toHaveTextContent('Dora');
  shown.rerender(<ResourceOwner owner={null} home={place} />);
  expect(screen.getByRole('link', { name: 'Research' })).toBeVisible();
  shown.rerender(<ResourceOwner owner={null} home={null} />);
  expect(shown.container).toHaveTextContent('System');
});

it('keeps one object mark in its page header and does not repeat it in the details card', () => {
  const base = planProvider();
  const provider = { ...base, access: { ...base.access, admin_access: reach } };
  const shown = show(<><PageHeader title={provider.name} mark={<Box />} adminAccess={reach} /><ProviderUsageCard provider={provider} /></>);
  expect(shown.container.querySelectorAll('[data-as-admin-mark]')).toHaveLength(1);
  shown.rerender(<ProviderUsageCard compact provider={provider} />);
  expect(shown.container.querySelectorAll('[data-as-admin-mark]')).toHaveLength(1);
});

it('adds an admin-only act’s context to its existing confirmation and an immediate act’s tooltip, without an extra confirmation', async () => {
  const run = vi.fn();
  const spec = adminSpec(rows[1], 'environment.manage');
  const shown = show(<><Button admin={spec} onClick={run}>Start</Button><AdminActNotice spec={adminSpec(rows[0], 'environment.manage')} /></>);
  await userEvent.hover(screen.getByRole('button', { name: 'Start' }));
  expect(await screen.findByRole('tooltip')).toHaveTextContent('As admin of Research');
  await userEvent.click(screen.getByRole('button', { name: 'Start' }));
  expect(run).toHaveBeenCalledOnce();
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  shown.rerender(<ConfirmDialog open onOpenChange={vi.fn()} title="Stop this environment?" confirmLabel="Stop" admin={spec} onConfirm={run}>Its turn stops.</ConfirmDialog>);
  expect(screen.getAllByRole('alertdialog')).toHaveLength(1);
  expect(within(screen.getByRole('alertdialog')).getByText('As admin of Research')).toBeVisible();
});

it('can explain an admin-only Manage act when View was ordinary, and leaves ordinary alternative permissions unmarked', () => {
  const ordinaryView = { access: { permissions: ['environment.view', 'environment.manage'], admin_access: null,
    admin_only: ['environment.manage'], admin_place: place } };
  show(<AdminActNotice spec={adminSpec(ordinaryView, 'environment.manage')} />);
  expect(screen.getByText('As admin of Research')).toBeVisible();
  const machine = { access: { permissions: ['machine.manage', 'host.manage'], admin_only: ['host.manage'] } };
  expect(isAdminAct(adminSpec(machine, ['machine.manage', 'host.manage']))).toBe(false);
  expect(isAdminAct(adminSpec({ access: { permissions: ['host.manage'], admin_only: ['host.manage'] } }, ['machine.manage', 'host.manage']))).toBe(true);
});
