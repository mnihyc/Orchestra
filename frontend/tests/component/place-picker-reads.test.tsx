/** A closed destination picker owns no tree read; opening it retains live choices and native filtering. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Nodes from '@/api/nodes';
import { orgsKey, type TreeReply } from '@/api/nodes';
import { PlacePicker } from '@/features/settings/PlacePicker';
import { i18n, setLocale } from '@/i18n';
import { treeWriter } from '@/realtime/stores/tree';
import { ACME, acmeTree, me, org, userRef } from '../doubles/fixtures';
import { show } from './support';

const reads = vi.hoisted(() => ({ tree: vi.fn() }));
vi.mock('@/api/nodes', async (load) => ({ ...await load<typeof Nodes>(), fetchTree: reads.tree }));
afterEach(async () => { cleanup(); treeWriter.clear(); vi.clearAllMocks(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`reads destinations only on opening and keeps the same choices on reopening in ${locale}`, async () => {
    await setLocale(locale);
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
    cache.setQueryData(orgsKey, [org(ACME.org, 'acme', 'Acme', userRef(me(1, 'ada', 'Ada')))]);
    const reply: TreeReply = { nodes: acmeTree(), hidden: [], seq: 1, since: null, complete: true };
    reads.tree.mockResolvedValue(reply);
    const changed = vi.fn(), user = userEvent.setup();
    show(<QueryClientProvider client={cache}><PlacePicker label={i18n.t('mcpSwitch.place')} value={null} onChange={changed} includeThreads /></QueryClientProvider>);
    expect(reads.tree).not.toHaveBeenCalled();
    const picker = screen.getByRole('combobox', { name: i18n.t('mcpSwitch.place') });
    await user.click(picker);
    await screen.findByRole('treeitem', { name: 'Acme / Research / Web / Fix login' });
    expect(reads.tree).toHaveBeenCalledTimes(1);
    await user.keyboard('{Escape}');
    await user.click(picker);
    await user.click(await screen.findByRole('treeitem', { name: 'Acme / Research / Web / Fix login' }));
    expect(changed).toHaveBeenCalledWith(expect.objectContaining({ id: ACME.fixLogin, kind: 'topic' }));
    expect(reads.tree).toHaveBeenCalledTimes(1);
  });

  it(`uses the loaded live tree and preserves the root and kind restrictions in ${locale}`, async () => {
    await setLocale(locale);
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    cache.setQueryData(orgsKey, [org(ACME.org, 'acme', 'Acme', userRef(me(1, 'ada', 'Ada')))]);
    treeWriter.reset(ACME.org, acmeTree(), 1);
    const changed = vi.fn(), user = userEvent.setup();
    show(<QueryClientProvider client={cache}><PlacePicker label={i18n.t('mcpSwitch.place')} value={null} onChange={changed} rootOrgId={ACME.org} only="topic" /></QueryClientProvider>);
    expect(reads.tree).not.toHaveBeenCalled();
    await user.click(screen.getByRole('combobox', { name: i18n.t('mcpSwitch.place') }));
    expect(await screen.findByRole('treeitem', { name: 'Acme / Research / Web / Fix login' })).toBeVisible();
    expect(screen.queryByRole('treeitem', { name: /^Acme \/ Web$/ })).not.toBeInTheDocument();
    expect(reads.tree).not.toHaveBeenCalled();
  });
}

it('chooses through the shared hierarchy with folding, keyboard search and no first-page cutoff', async () => {
  await setLocale('en');
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(orgsKey, [org(ACME.org, 'acme', 'Acme', userRef(me(1, 'ada', 'Ada')))]);
  const base = acmeTree().find((node) => node.id === ACME.org);
  if (base === undefined) throw new Error('The fixture has a root.');
  const nodes = [base, ...Array.from({ length: 45 }, (_, index) => ({ ...base, id: `project-${index}`, kind: 'project' as const, name: `Project ${String(index).padStart(2, '0')}`, parent_id: base.id }))];
  treeWriter.reset(ACME.org, nodes, 1);
  const changed = vi.fn(), user = userEvent.setup();
  show(<QueryClientProvider client={cache}><PlacePicker label="Place" value={null} onChange={changed} /></QueryClientProvider>);
  await user.click(screen.getByRole('combobox', { name: 'Place' }));
  const tree = await screen.findByRole('tree', { name: 'Place' });
  expect(within(tree).getAllByRole('treeitem')).toHaveLength(46);
  expect(within(tree).getByRole('treeitem', { name: 'Acme / Project 44' })).toBeVisible();
  const root = within(tree).getByRole('treeitem', { name: 'Acme' });
  root.focus();
  await user.keyboard('{ArrowLeft}');
  expect(within(tree).getAllByRole('treeitem')).toHaveLength(1);
  await user.keyboard('{ArrowRight}');
  expect(within(tree).getAllByRole('treeitem')).toHaveLength(46);
  await user.type(screen.getByRole('searchbox', { name: i18n.t('settings.place.placeholder') }), 'Project 44');
  expect(within(tree).getAllByRole('treeitem')).toHaveLength(2);
  await user.keyboard('{ArrowDown}{End}{Enter}');
  expect(changed).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ id: 'project-44' }));
  expect(reads.tree).not.toHaveBeenCalled();
});
