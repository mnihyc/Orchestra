/** The actual trail keeps its filters open and translates its protocol actions and setting values. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { infiniteQueryOptions } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Router from '@tanstack/react-router';
import type * as TraceApi from '@/api/trace';
import * as Users from '@/api/users';
import { client } from '@/api/client';
import { TraceFilters } from '@/features/trace/Filters';
import { ACTIONS } from '@/features/trace/catalogue';
import { RequestRow } from '@/features/trace/Requests';
import { Trail } from '@/features/trace/Trail';
import { auditActionName } from '@/features/trace/words';
import type { AuditEntry, AuditEntryPage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { readViewChoice, writeViewChoice } from '@/lib/viewChoices';
import { show } from './support';

const route = vi.hoisted(() => { const search: Record<string, unknown> = {}; return { replace: vi.fn(), search }; });
vi.mock('@tanstack/react-router', async (original) => {
  const actual = await original<typeof Router>();
  return { ...actual,
    useLocation: () => ({ pathname: '/admin/audit', search: route.search }),
    useRouter: () => ({ history: { replace: route.replace } }),
    useNavigate: () => (options: { to: string; search: Record<string, unknown> }) => { route.replace(options.to + actual.defaultStringifySearch(options.search)); return Promise.resolve(); },
  };
});
vi.mock('@/api/trace', async (original) => ({ ...await original<typeof TraceApi>(),
  trailQuery: () => infiniteQueryOptions({ queryKey: ['readable-trail'], initialPageParam: null as string | null,
    queryFn: (): Promise<AuditEntryPage> => Promise.resolve({ items: [], next_cursor: null }), getNextPageParam: () => undefined }),
}));

const session = client.session.getState();
const appearance = useAppearance.getState();
const themeBefore = document.documentElement.getAttribute('data-theme');
const matchMediaBefore = Object.getOwnPropertyDescriptor(window, 'matchMedia');
const userId = 'readable-audit-test';
beforeEach(() => {
  Object.defineProperty(window, 'matchMedia', { configurable: true, writable: true, value: vi.fn((media: string) => ({
    matches: true, media, onchange: null, addListener: vi.fn(), removeListener: vi.fn(),
    addEventListener: vi.fn(), removeEventListener: vi.fn(), dispatchEvent: vi.fn(() => true),
  })) });
  route.search = {}; route.replace.mockClear();
  client.session.setState({ status: 'signed_in', userId });
  writeViewChoice(userId, 'page.trace.trace', 'filters', 'false');
  writeViewChoice(userId, 'trace', 'requestDetails', false, 'setting-row');
});
afterEach(async () => {
  cleanup();
  if (matchMediaBefore === undefined) Reflect.deleteProperty(window, 'matchMedia');
  else Object.defineProperty(window, 'matchMedia', matchMediaBefore);
  client.session.setState(session); useAppearance.setState(appearance); await setLocale('en');
  if (themeBefore === null) document.documentElement.removeAttribute('data-theme');
  else document.documentElement.dataset.theme = themeBefore;
});

function settingEntry(): AuditEntry {
  return { id: 'setting-entry', at: '2026-10-15T12:00:00Z', actor: { kind: 'system' }, as_admin: false,
    action: 'settings.updated', kind: 'changed', request_id: 'one-request', event_deletion_id: null,
    object: { id: 'thread', kind: 'thread', name: 'Repair login', state: 'live', deletion_id: null,
      deleted_at: null, deleted_by: null, purge_at: null, purged_at: null, can_open: false, can_inspect: false, can_restore: false },
    node: null, override: false, history: true, ip: null, summary: 'Approval mode set to on_request',
    details: { key: 'approval.mode', value: 'on_request', was: 'ask', locked: false, revision: 3 } };
}

// Draws every row and types through it as a person would: 4 to 15 s on a busy machine, so 30 s as the suite's other long cases.
it('locates the last complete catalogue action locally and retains an unknown current filter and All', { timeout: 30_000 }, async () => {
  const user = userEvent.setup(), change = vi.fn();
  const people = vi.spyOn(Users, 'searchUsers').mockResolvedValue([]);
  const actions = Object.values(ACTIONS).flat();
  const last = actions.at(-1);
  if (last === undefined) throw new Error('The action catalogue is empty');
  try {
    show(<TraceFilters scope={{ type: 'system', name: 'System' }} filters={{ action: 'future.reindexed' }} person={null} onChange={change} />);
    const input = screen.getByRole('combobox', { name: i18n.t('trace.action') });
    expect(input).toHaveValue(i18n.t('trace.unrecognizedAct'));
    await user.click(input);
    expect(actions.length).toBeGreaterThan(10);
    expect(screen.getAllByRole('option')).toHaveLength(actions.length + 2);
    await user.type(input, last);
    expect(screen.getAllByRole('option')).toHaveLength(1);
    expect(change).not.toHaveBeenCalled(); expect(people).not.toHaveBeenCalled();
    await user.keyboard('{Escape}'); expect(input).toHaveValue(i18n.t('trace.unrecognizedAct'));
    await user.click(input); await user.type(input, last); await user.keyboard('{Enter}');
    expect(change).toHaveBeenCalledExactlyOnceWith({ action: last });
    await user.click(input); await user.type(input, i18n.t('trace.allActions'));
    await user.click(screen.getByRole('option', { name: i18n.t('trace.allActions') }));
    expect(change).toHaveBeenLastCalledWith({ action: undefined });
    expect(people).not.toHaveBeenCalled();
  } finally { people.mockRestore(); }
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`remembers the Filters disclosure across visits in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme }); document.documentElement.dataset.theme = theme;
    const first = show(<Trail scope={{ type: 'system', name: 'System' }} embedded />);
    const summary = screen.getByText(i18n.t('trace.filters'), { exact: true });
    expect(summary.closest('details')).not.toHaveAttribute('open');
    await userEvent.click(summary);
    await waitFor(() => expect(summary.closest('details')).toHaveAttribute('open'));
    await waitFor(() => expect(readViewChoice(userId, 'page.trace.trace', 'filters', 'false')).toBe('true'));
    for (const key of ['person', 'kind', 'from', 'to', 'action'] as const) expect(screen.getByLabelText(i18n.t(`trace.${key}`))).toBeVisible();
    expect(screen.queryByText('More', { exact: true })).not.toBeInTheDocument();
    first.unmount();
    show(<Trail scope={{ type: 'system', name: 'System' }} embedded />);
    expect(screen.getByText(i18n.t('trace.filters'), { exact: true }).closest('details')).toHaveAttribute('open');
  });

  it(`names an exact action but sends the original filter in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme }); document.documentElement.dataset.theme = theme;
    const change = vi.fn();
    const view = show(<TraceFilters scope={{ type: 'system', name: 'System' }} filters={{ kind: 'made' }} person={null} onChange={change} />);
    const action = screen.getByRole('combobox', { name: i18n.t('trace.action') });
    await userEvent.click(action);
    // Its first opening draws every action of the catalogue, which can take more than a second on a busy machine.
    await userEvent.click(await screen.findByRole('option', { name: auditActionName('thread.created', i18n.t) }, { timeout: 5_000 }));
    expect(change).toHaveBeenCalledWith({ kind: 'made', action: 'thread.created' });
    expect(view.container).not.toHaveTextContent('thread.created');
    view.unmount();
    show(<TraceFilters scope={{ type: 'system', name: 'System' }} filters={{ action: 'future.reindexed', kind: 'future_kind' }} person={null} onChange={change} />);
    expect(screen.getByRole('combobox', { name: i18n.t('trace.action') })).toHaveValue(i18n.t('trace.unrecognizedAct'));
    expect(screen.getByRole('combobox', { name: i18n.t('trace.kind') })).toHaveTextContent(i18n.t('trace.kinds.unknown'));
    expect(screen.queryByText('future.reindexed')).not.toBeInTheDocument();
    expect(screen.queryByText('future_kind')).not.toBeInTheDocument();
  });

  it(`shows one readable setting sentence and only additional details in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme }); document.documentElement.dataset.theme = theme;
    const entry = settingEntry();
    const view = show(<ul><RequestRow line={{ key: 'setting-row', chosen: entry, rows: [entry], mayContinue: false }} select={false} selected={new Set<string>()} onSelect={vi.fn()} onRestore={vi.fn()} /></ul>);
    const sentence = i18n.t('trace.sentenceSettingSet', { actor: i18n.t('trace.systemActor'), setting: i18n.t('settingRegistry.approval.mode.title'), value: i18n.t('choiceCatalog.approvalMode.on_request.name') });
    expect(screen.getAllByText(sentence, { exact: true })).toHaveLength(1);
    expect(screen.getAllByText(i18n.t('trace.systemActor'),{exact:true})).toHaveLength(1);
    expect(view.container).not.toHaveTextContent('settings.updated');
    expect(view.container).not.toHaveTextContent('on_request');
    expect(view.container).not.toHaveTextContent(/matching rows?|匹配记录/);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('trace.showDetails', { action: auditActionName(entry.action, i18n.t) }) }));
    const details = view.container.querySelector('[data-trace-detail]');
    if (!(details instanceof HTMLElement)) throw new Error('The recorded detail did not open');
    expect(within(details).getByText(i18n.t('choiceCatalog.approvalMode.ask.name'), { exact: true })).toBeVisible();
    expect(within(details).getByText(i18n.t('everything.kinds.thread'), { exact: true })).toBeVisible();
    expect(details).not.toHaveTextContent('approval.mode');
    expect(details).not.toHaveTextContent('on_request');
  });
}
