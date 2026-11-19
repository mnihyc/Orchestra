/** A tag's project chooser uses readable projects, the common place labels and the real List filter. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { TreeReply } from '@/api/nodes';
import { TagProjects } from '@/features/finder/TagProjects';
import type { HiddenItem, TreeNode } from '@/generated/api-types';

import { show } from './support';

const { read } = vi.hoisted(() => ({ read: vi.fn<() => Promise<TreeReply>>() }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), fetchTree: read }));
afterEach(() => { vi.clearAllMocks(); });

function node(id: string, kind: TreeNode['kind'], parent_id: string | null, name = id): TreeNode {
  return { id, kind, parent_id, name, participating: true, address: kind === 'org' && parent_id === null ? name.toLowerCase() : null, color: null, custom_color: id === 'web-research' ? '#123456' : null, archived: false, deleted: false, restricted: false, docs: 0, chat: { id: `chat-${id}`, seq: 0 }, thread: null, topic: null, tree_seq: 1 };
}

const TAG = { id: 'tag', name: 'Urgent', node: { id: 'acme', kind: 'org', name: 'Acme', redacted: false } } as const;
const PRIVATE: HiddenItem = { id: 'private', parent_id: 'acme', kind: 'project', name: 'A private folder name', owner: { id: 'owner', kind: 'human', username: 'owner', display_name: 'Owner', avatar: { text: 'O', color: 1 } }, content_hidden: true, private: true };
const TREE: TreeReply = { complete: true, since: null, seq: 1, hidden: [PRIVATE], nodes: [node('acme', 'org', null, 'Acme'), node('research', 'org', 'acme', 'Research'), node('labs', 'org', 'acme', 'Labs'), node('web-research', 'project', 'research', 'Web'), node('web-labs', 'project', 'labs', 'Web'), node('readable-child', 'project', 'private', 'Readable child'), node('elsewhere', 'org', null, 'Elsewhere'), node('outside', 'project', 'elsewhere', 'Outside')] };

function draw() {
  const choose = vi.fn();
  const close = vi.fn();
  show(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><TagProjects tag={TAG} rootOrgId="acme" onChoose={choose} onClose={close} /></QueryClientProvider>);
  return { choose, close };
}

describe('choosing where to use a found tag', () => {
  it('tells repeated project names apart, omits private/outside choices and opens the exact tag array by keyboard', async () => {
    read.mockResolvedValue(TREE);
    const { choose } = draw();
    const project = await screen.findByRole('button', { name: 'Web · Research' });
    expect(screen.getByRole('button', { name: 'Web · Labs' })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Readable child' })).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Outside' })).not.toBeInTheDocument();
    expect(screen.queryByText('A private folder name')).not.toBeInTheDocument();
    expect(project.querySelector('[data-node-icon]')).toHaveAttribute('style', expect.stringContaining('#123456'));
    project.focus();
    await userEvent.keyboard('{Enter}');
    expect(choose).toHaveBeenCalledWith({ to: '/project/$projectId', params: { projectId: 'web-research' }, search: { view: 'list', tag: JSON.stringify(['tag']) } });
  });

  it('can narrow by the distinguishing parent name and keeps Cancel separate from choosing', async () => {
    read.mockResolvedValue(TREE);
    const { choose, close } = draw();
    await screen.findByRole('button', { name: 'Web · Research' });
    await userEvent.type(screen.getByRole('searchbox'), 'Labs');
    expect(screen.queryByRole('button', { name: 'Web · Research' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Web · Labs' })).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(close).toHaveBeenCalledOnce();
    expect(choose).not.toHaveBeenCalled();
  });

  it('shows a failed read with Retry and never offers stale project choices', async () => {
    read.mockRejectedValueOnce(new Error('The tree could not be read.')).mockResolvedValue(TREE);
    draw();
    await userEvent.click(await screen.findByRole('button', { name: /Try again/ }));
    expect(await screen.findByRole('button', { name: 'Web · Research' })).toBeVisible();
    expect(screen.queryByText('A private folder name')).not.toBeInTheDocument();
  });
});
