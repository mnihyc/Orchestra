/** One visible audience follows native page grants; it never changes a personal-read option. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as Access from '@/api/access';
import { client } from '@/api/client';
import type { Holder, PermissionPage, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { AudienceBanner } from '@/features/thread/AudienceBanner';
import { threadAudience } from '@/features/thread/audience';
import { ADA, BEA, CATALOGUE, TOPIC, named, pageFor } from '../unit/general-access-fixtures';
import { show } from './support';

const source = vi.hoisted(() => ({ page: null as PermissionPage | null, failed: false, read: vi.fn(), share: vi.fn() }));
vi.mock('@/api/access', async (load) => ({ ...await load<typeof Access>(),
  pageQuery: (target: { id: string }) => ({ queryKey: ['page', 'nodes', target.id], retry: false,
    queryFn: () => { source.read(); return source.failed ? Promise.reject(new Error('Audience refused')) : Promise.resolve(source.page); } }),
  permissionsQuery: { queryKey: ['permissions'], queryFn: () => Promise.resolve(CATALOGUE.reply) },
}));
vi.mock('@/features/access/AccessDialog', () => ({ openAccess: source.share }));

const before = client.session.getState();
beforeEach(() => { source.failed = false; source.read.mockClear(); source.share.mockClear(); client.session.setState({ userId: ADA.id }); });
afterEach(async () => { cleanup(); client.session.setState(before); await setLocale('en'); });

function privatePage(): PermissionPage {
  const page = pageFor('thread');
  return { ...page, rows: page.rows.map((row) => ({ ...row, inherit: false, inherited: null, entries: [] })) };
}
function grant(page: PermissionPage, permission: string, holder: Holder, n = 2): PermissionPage {
  return { ...page, revision: page.revision + 1, rows: page.rows.map((row) => row.permission === permission ? { ...row, entries: [...row.entries, named(holder, n)] } : row) };
}
function draw(page: PermissionPage) {
  source.page = page;
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const thread: Pick<Thread, 'id' | 'name' | 'owner'> = { id: page.object.id, name: page.object.name, owner: ADA };
  const view = show(<QueryClientProvider client={cache}><AudienceBanner thread={thread} /></QueryClientProvider>);
  return { ...view, cache, thread };
}
async function refresh(cache: QueryClient, page: PermissionPage) {
  source.page = page;
  await act(async () => { await cache.invalidateQueries({ queryKey: ['page'] }); });
}

describe('the existing native permission-page projection', () => {
  it('counts empty groups and future roles/agents as named audiences, never as zero people', () => {
    const holders: Holder[] = [
      { kind: 'group', group: { id: 'empty-group', org_id: TOPIC.id, name: 'Reviewers', description: '', member_count: 0, revision: 1 } },
      { kind: 'role', role: 'viewer', at: TOPIC, count: 0 },
      { kind: 'agents', node: TOPIC }, { kind: 'members', node: TOPIC },
      { kind: 'node', node: TOPIC }, { kind: 'everyone' },
    ];
    let page = privatePage();
    holders.forEach((holder, at) => { page = grant(page, 'thread.view', holder, at + 2); });
    const audience = threadAudience(page, CATALOGUE, ADA.id, 1);
    expect(audience.map((one) => one.holder.kind)).toEqual(holders.map((holder) => holder.kind));
    expect(audience.every((one) => !one.sends)).toBe(true);
  });

  it('uses native locks and inheritance, includes another owner and deduplicates stronger grants', () => {
    const one = { kind: 'user' as const, user: BEA };
    const page = grant(grant(privatePage(), 'thread.view', one), 'thread.interact', one, 3);
    expect(threadAudience(page, CATALOGUE, ADA.id, 1)).toEqual([expect.objectContaining({ sends: true, holder: one })]);
    page.rows = page.rows.map((row) => row.permission === 'thread.interact' || row.permission === 'thread.view'
      ? { ...row, locked: { by: TOPIC, here: false }, inherited: { holders: [], from: TOPIC } } : row);
    expect(threadAudience(page, CATALOGUE, ADA.id, 1)).toEqual([]);
    page.owners.push(named(one, 4));
    expect(threadAudience(page, CATALOGUE, ADA.id, 1)).toEqual([expect.objectContaining({ sends: true, holder: one })]);
  });

  it('drops expired grants while retaining an active weaker grant and known inherited entries', () => {
    const page = grant(grant(privatePage(), 'thread.view', { kind: 'user', user: BEA }), 'thread.interact', { kind: 'user', user: BEA }, 3);
    const at = Date.parse('2030-01-01T00:00:00Z');
    page.rows = page.rows.map((row) => row.permission === 'thread.interact' ? { ...row, entries: row.entries.map((entry) => ({ ...entry, expires_at: '2030-01-01T00:00:00Z' })) } : row);
    expect(threadAudience(page, CATALOGUE, ADA.id, at - 1)[0]?.sends).toBe(true);
    expect(threadAudience(page, CATALOGUE, ADA.id, at)[0]?.sends).toBe(false);
    page.rows = page.rows.map((row) => row.permission === 'thread.view' ? { ...row, entries: [], inherit: true,
      inherited: { holders: [{ kind: 'user', user: BEA }], entries: [named({ kind: 'user', user: BEA }, 5, { expires_at: '2030-01-01T00:00:00Z' })], from: TOPIC } } : row);
    expect(threadAudience(page, CATALOGUE, ADA.id, at)).toEqual([]);
  });
});

for (const locale of ['en', 'zh-CN'] as const) describe(`thread audience in ${locale}`, () => {
  beforeEach(async () => { await setLocale(locale); });

  it('hides an owner-only audience and updates add, sender and removal through the native page cache', async () => {
    const page = privatePage(); const { cache } = draw(page);
    await waitFor(() => expect(source.read).toHaveBeenCalled());
    await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument());
    await refresh(cache, grant(page, 'thread.view', { kind: 'user', user: BEA }));
    const line = await screen.findByRole('status');
    expect(line).toHaveTextContent(i18n.t('thread.audience.personal'));
    expect(line).not.toHaveTextContent(i18n.t('thread.audience.canSend'));
    await refresh(cache, grant(page, 'thread.manage', { kind: 'user', user: BEA }));
    expect(await screen.findByRole('status')).toHaveTextContent(i18n.t('thread.audience.canSend'));
    await refresh(cache, page);
    await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument());
  });

  it('opens the shared people list and existing sharing dialog by keyboard', async () => {
    const { thread } = draw(grant(privatePage(), 'thread.view', { kind: 'user', user: BEA }));
    const user = userEvent.setup();
    const trigger = await screen.findByRole('button', { name: i18n.t('thread.audience.shared', { audience: i18n.t('thread.audience.people', { count: 1, number: '1' }) }) });
    act(() => trigger.focus()); await user.keyboard('{Enter}');
    const card = await screen.findByRole('dialog', { name: i18n.t('thread.audience.title') });
    expect(within(card).getByText(BEA.display_name)).toBeVisible();
    await user.keyboard('{Escape}');
    await user.click(screen.getByRole('button', { name: i18n.t('thread.audience.sharing') }));
    expect(source.share).toHaveBeenCalledExactlyOnceWith({ kind: 'thread', id: thread.id, name: thread.name }, { share: true });
  });

  it('names an empty future agent audience even with no personal-read option input', async () => {
    draw(grant(privatePage(), 'thread.view', { kind: 'agents', node: TOPIC }));
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent(i18n.t('access.holder.agentsOf', { place: TOPIC.name })));
    expect(screen.getByRole('status')).toHaveTextContent(i18n.t('thread.audience.personal'));
  });

  it('uses owner wording for another reader and removes stale names after a refused refresh', async () => {
    client.session.setState({ userId: BEA.id });
    const page = grant(privatePage(), 'thread.view', { kind: 'user', user: BEA });
    const { cache } = draw(page);
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent(i18n.t('thread.audience.owner')));
    source.failed = true; await refresh(cache, page);
    expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('thread.audience.unavailable'));
    expect(screen.queryByText(BEA.display_name)).not.toBeInTheDocument();
    source.failed = false; source.page = privatePage();
    await userEvent.setup().click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
    await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument());
  });

  it('does not lend one thread’s audience to a different target while it loads', async () => {
    const first = grant(privatePage(), 'thread.view', { kind: 'user', user: BEA });
    const view = draw(first);
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent(i18n.t('thread.audience.personal')));
    const other = { ...view.thread, id: 'different-thread', name: 'Different thread' };
    view.rerender(<QueryClientProvider client={view.cache}><AudienceBanner thread={other} /></QueryClientProvider>);
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent(i18n.t('thread.audience.loading')));
    expect(screen.queryByText(i18n.t('thread.audience.personal'))).not.toBeInTheDocument();
    const page = privatePage(); page.object = { ...page.object, id: other.id, name: other.name };
    await refresh(view.cache, page);
    await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument());
  });
});
