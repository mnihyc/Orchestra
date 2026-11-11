/** A closed destination picker owns no tree read; opening it retains live choices and native filtering. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, screen } from '@testing-library/react';
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
    await screen.findByRole('option', { name: 'Acme / Research / Web / Fix login' });
    expect(reads.tree).toHaveBeenCalledTimes(1);
    await user.keyboard('{Escape}');
    await user.click(picker);
    await user.click(await screen.findByRole('option', { name: 'Acme / Research / Web / Fix login' }));
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
    expect(await screen.findByRole('option', { name: 'Acme / Research / Web / Fix login' })).toBeVisible();
    expect(screen.queryByRole('option', { name: /^Acme \/ Web$/ })).not.toBeInTheDocument();
    expect(reads.tree).not.toHaveBeenCalled();
  });
}
