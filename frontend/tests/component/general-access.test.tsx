/** The actual new-thread and Share forms use one audience choice; an inherited holder is edited here. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type * as AccessApi from '@/api/access';
import type * as AuthApi from '@/api/auth';
import type * as NodesApi from '@/api/nodes';
import type * as ResourcesApi from '@/api/resources';
import type * as AddForm from '@/features/access/AddHolders';

import { ChangePreview, type Asked } from '@/features/access/ChangePreview';
import type { Changes } from '@/features/access/changes';
import { GeneralAccessControl } from '@/features/access/GeneralAccess';
import { ShortForm } from '@/features/access/ShortForm';
import { NewThreadDialog } from '@/features/start/BindingDialog';
import { NO_PICKS, resolve } from '@/features/start/choices';
import type { PageChange } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';

import { CATALOGUE, pageFor, TOPIC } from '../unit/general-access-fixtures';
import { options } from '../unit/start-fixtures';
import { show } from './support';

const { start } = vi.hoisted(() => ({ start: vi.fn() }));
const fixture = vi.hoisted((): { defaultsKind: AccessApi.AccessKind } => ({ defaultsKind: 'thread' }));
const answer = options();
vi.mock('@/features/start/options', () => ({ useStartOptions: () => ({ options: answer, resolved: resolve(answer, NO_PICKS, { suggest: true }), loading: false, failed: null, refreshing: false, retry: () => undefined }) }));
vi.mock('@/features/start/start', () => ({ useStart: () => ({ start, pending: false, error: null, standing: { topic: null, environment: null }, reset: () => undefined, step: null }) }));
vi.mock('@/features/start/Where', () => ({ Where: () => null, WhereOptions: () => null }));
vi.mock('@/features/resources/providers/ProviderUsageCard', () => ({ ProviderUsageCard: () => null }));
vi.mock('@/api/access', async (original) => {
  const actual = await original<typeof AccessApi>();
  return { ...actual, permissionsQuery: { queryKey: ['permissions'], queryFn: () => Promise.resolve(CATALOGUE.reply) },
    defaultsQuery: (id: string) => ({ queryKey: ['defaults', id], queryFn: () => Promise.resolve(pageFor(fixture.defaultsKind)) }),
    readPage: () => Promise.resolve(pageFor()), previewPage: () => Promise.resolve({ pages: 1, orgs: null, gaining: [], losing: [], more: 0 }) };
});
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof ResourcesApi>(), providersQuery: () => ({ queryKey: ['providers'], queryFn: () => Promise.resolve([]) }), machinesQuery: { queryKey: ['machines'], queryFn: () => Promise.resolve([]) } }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof NodesApi>(), nodeQuery: (id: string) => ({ queryKey: ['node', id], queryFn: () => Promise.resolve({ org_id: 'org', access: { permissions: ['topic.manage'] } }) }) }));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof AuthApi>(), meQuery: { queryKey: ['me'], queryFn: () => Promise.resolve({ id: 'me' }) } }));
vi.mock('@/features/access/AddHolders', async (original) => ({ ...await original<typeof AddForm>(), AddHolders: () => null }));
vi.mock('@/features/access/Owners', () => ({ Owners: () => null }));
vi.mock('@/features/access/GoesWith', () => ({ GoesWith: () => null }));
vi.mock('@/features/access/RequestCard', () => ({ WaitingRequests: () => null }));
vi.mock('@/features/access/RowCell', () => ({ FollowMark: () => null, RowCell: () => null }));
vi.mock('@/features/access/Reaches', () => ({ EnvironmentReachLine: () => null, ReachesLine: () => null, privateWarning: () => null }));
vi.mock('@/features/navigation/placeLabels', () => ({ usePlaceLabel: () => (place: { name?: string | null } | null | undefined) => place?.name ?? null, useKnownPlaces: () => new Map() }));

beforeEach(() => { start.mockClear(); fixture.defaultsKind = 'thread'; });

describe('one general access choice', () => {
  it.each(['org', 'project', 'topic'] as const)('lists a %s’s holder levels and provenance without repeating a defaults paragraph', async (kind) => {
    fixture.defaultsKind = kind;
    const changes: Changes = { busy: false, doing: null, error: null, stale: null, forget: () => undefined, again: () => Promise.resolve(false), run: () => Promise.resolve(false) };
    try {
      for (const locale of ['en', 'zh-CN'] as const) {
        await setLocale(locale);
        const page = pageFor(kind);
        const target = { kind, id: page.object.id, name: 'Example' };
        const view = show(<ShortForm target={target} page={page} catalogue={CATALOGUE} source={{ type: 'page', target }} changes={changes} ask={() => undefined} onFull={() => undefined} onViewAs={() => undefined} />);
        expect(screen.getByRole('region', { name: i18n.t('access.roster.title') })).toBeVisible();
        expect(screen.getByRole('group', { name: i18n.t('access.choice.manage') })).toBeVisible();
        expect(screen.getAllByText(i18n.t('access.roster.inheritedFrom', { place: 'Fix login' }))).not.toHaveLength(0);
        expect(screen.queryByRole('button', { name: i18n.t('access.footer.viewAs') })).not.toBeInTheDocument();
        expect(view.container.textContent).not.toMatch(/can\s+can\b/i);
        expect(view.container.textContent).not.toContain('可以可以');
        expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
        view.unmount();
      }
    } finally { await setLocale('en'); }
  });

  it('is in the actual new-thread dialog, starts on the topic’s rules, and sends the chosen level and readonly tick', async () => {
    const user = userEvent.setup();
    show(<NewThreadDialog parent={{ id: TOPIC.id, name: 'Fix login', kind: 'topic', parentId: 'project', archived: false, status: 'open', folder: false }} open onClose={() => undefined} />);
    await user.click(screen.getByRole('button', { name: 'Options' }));
    const choice = await screen.findByRole('combobox', { name: 'General access' });
    expect(choice).toHaveTextContent('Defaults');
    expect(screen.getByRole('checkbox', { name: 'Let them into the environment' })).toBeChecked();
    await user.click(choice);
    await user.keyboard('{ArrowDown}');
    const viewChoice = await screen.findByRole('option', { name: 'View' });
    expect(viewChoice).toHaveAccessibleDescription('Everyone in Fix login can view');
    await user.click(viewChoice);
    await user.click(screen.getByRole('button', { name: 'Create thread' }));
    expect(start).toHaveBeenCalledWith(expect.objectContaining({ generalAccess: 'view', privately: false, letIntoEnvironment: true }));
  });

  it.each([['document', 'Edit', 'Everyone in Fix login can edit'], ['chat', 'Write', 'Everyone in Fix login can write'], ['provider', 'Use', 'Everyone in Fix login can use'], ['org', 'Take part', 'Every member of Fix login can take part']] as const)('uses %s’s short term with its audience described after it', async (kind, term, words) => {
    const user = userEvent.setup();
    show(<GeneralAccessControl catalogue={CATALOGUE} kind={kind} place="Fix login" value="defaults" onChange={() => undefined} />);
    await user.click(screen.getByRole('combobox', { name: 'General access' }));
    await user.keyboard('{ArrowDown}');
    const option = await screen.findByRole('option', { name: term });
    expect(option).toBeVisible();
    expect(option).toHaveAccessibleDescription(words);
  });

  it('names an org’s own members but follows its parent’s rules', async () => {
    const user = userEvent.setup();
    show(<GeneralAccessControl catalogue={CATALOGUE} kind="org" place="Acme" rulesPlace="the system" value="defaults" onChange={() => undefined} />);
    expect(screen.getByRole('combobox', { name: 'General access' })).toHaveTextContent('Defaults');
    await user.click(screen.getByRole('combobox', { name: 'General access' }));
    await user.keyboard('{ArrowDown}');
    expect(await screen.findByRole('option', { name: 'View' })).toHaveAccessibleDescription('Every member of Acme can view');
  });

  it('changes a followed Take part role in Share, after one confirmation, and offers the way back to the place’s rules', async () => {
    const user = userEvent.setup();
    const writes: PageChange[] = [];
    function Share() {
      const [page, setPage] = useState(pageFor());
      const [asked, ask] = useState<Asked | null>(null);
      const target = { kind: 'thread' as const, id: page.object.id, name: 'Example' };
      const source = { type: 'page' as const, target };
      const changes: Changes = { busy: false, doing: null, error: null, stale: null, forget: () => undefined, again: () => Promise.resolve(false), run: (request) => {
        writes.push(request.make(page));
        setPage({ ...page, revision: 8, state: 'custom', rows: page.rows.map((row) => ['chat.interact', 'doc.edit', 'thread.interact'].includes(row.permission) ? { ...row, inherit: false, inherited: null } : row) });
        return Promise.resolve(true);
      } };
      return <><ShortForm target={target} page={page} catalogue={CATALOGUE} source={source} changes={changes} ask={ask} onFull={() => undefined} onViewAs={() => undefined} /><ChangePreview source={source} asked={asked} changes={changes} onClose={() => ask(null)} /></>;
    }
    show(<Share />);
    const takingPart = screen.getByRole('group', { name: 'Can interact' });
    await user.click(within(takingPart).getByRole('button', { name: i18n.t('access.general.changeHolder', { name: 'Interactors of Example' }) }));
    await user.click(screen.getByRole('menuitem', { name: 'Take Interactors of Example out' }));
    expect(writes).toEqual([]);
    const confirmation = screen.getByRole('alertdialog', { name: 'Take Interactors of Example out?' });
    await user.click(within(confirmation).getByRole('button', { name: 'Take out Interactors of Example' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    expect(writes).toHaveLength(1);
    expect(writes[0]?.rows?.map((row) => row.permission).sort()).toEqual(['chat.interact', 'doc.edit', 'thread.interact']);
    expect(writes[0]?.rows?.every((row) => row.inherit === false)).toBe(true);
    expect(screen.getByRole('combobox', { name: 'General access' })).toHaveTextContent('Custom');
    expect(screen.getByRole('button', { name: 'Back to Fix login’s rules' })).toBeVisible();
    expect(screen.getByRole('group', { name: 'Can view' })).toHaveTextContent('Everyone in Example');
  });
});
