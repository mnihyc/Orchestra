/** Search by meaning in Management: each org's state in the reader's language, progress with the time left, the disk against its share, and the two choices a waiting index offers with the size and time shown first. */
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';

import { SemanticSearch } from '@/features/admin/SemanticSearch';
import type { SemanticSearchOrg, SemanticSearchOverview } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { formatBytes, formatTokens } from '@/lib/format';
import { formatSpan } from '@/lib/time';
import { show } from './support';

const calls = vi.hoisted(() => ({
  templateId: null as string | null,
  overview: vi.fn(),
  rebuild: vi.fn(),
  keep: vi.fn(),
  local: vi.fn(),
  put: vi.fn(),
}));
vi.mock('@/api/admin', () => ({
  semanticSearchKey: ['admin', 'semantic-search'],
  semanticSearchQuery: { queryKey: ['admin', 'semantic-search'], queryFn: calls.overview },
  rebuildSemanticSearch: calls.rebuild,
  keepSemanticSearch: calls.keep,
}));
vi.mock('@/api/settings', () => ({ settingsKey: (id: string) => ['settings', id], putSetting: calls.put }));
// The system's own keys are drawn by the settings editor every place uses; its own tests cover it.
vi.mock('@/features/settings/Editor', () => ({ SettingsEditor: () => <div data-testid="settings-editor" /> }));

vi.mock('@/features/resources/providers/AddProvider', () => ({AddProviderDialog: (props: {initialHome: string; initialPreset: string}) => <div role="dialog" aria-label={props.initialPreset}>{props.initialHome}</div>}));
vi.mock('@/api/resources', () => ({providerQuery: (id: string) => ({queryKey: ['provider', id], queryFn: () => Promise.resolve({runtime_config: calls.templateId === null ? {} : {'runtime.template_id': calls.templateId}})}), templateQuery: (id: string) => ({queryKey: ['template', id], queryFn: () => Promise.resolve({id, name: 'Local embeddings'})})}));

afterEach(() => { calls.templateId = null; vi.clearAllMocks(); });

const org = (id: string, name: string, over: Partial<SemanticSearchOrg>): SemanticSearchOrg => ({
  org: { id, kind: 'org', name, redacted: false },
  enabled: true,
  provider_id: '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b01',
  model_name: 'embeddinggemma-2',
  dimensions: 256,
  state: 'idle',
  reason: null,
  detail: null,
  active: null,
  building: null,
  tokens: 0,
  pending_tokens: 0,
  tokens_per_second: null,
  seconds_left: null,
  content_bytes: 0,
  used_bytes: 0,
  allowed_bytes: null,
  rebuild_bytes: null,
  rebuild_seconds: null,
  ...over,
});

const active = {
  id: '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5c01',
  provider_id: '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b09',
  model_name: 'old-model',
  asked_dimensions: 256,
  dimensions: 256,
  query_prefix: 'query: ',
  document_prefix: 'passage: ',
};

const overview = (available = true): SemanticSearchOverview => ({
  available,
  unavailable_reason: available ? null : 'absent',
  interval_secs: 10,
  orgs: [
    org('0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01', 'Acme', {
      state: 'indexing', reason: 'more', tokens: 4_000, pending_tokens: 1_000, tokens_per_second: 200, seconds_left: 5,
      content_bytes: 2_000_000, used_bytes: 1_000_000, allowed_bytes: 2_000_000, active,
    }),
    org('0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a02', 'Bureau', {
      state: 'waiting', reason: 'choice', tokens: 1_000, active, rebuild_bytes: 3_000_000, rebuild_seconds: 120,
    }),
    org('0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a03', 'Cafe', { state: 'not_named', provider_id: null, model_name: null, dimensions: null }),
  ],
});

for (const locale of ['en', 'zh-CN'] as const) for (const tokens of [0, 4000]) it.each(['provider_disabled', 'provider_removed'] as const)(`keeps the reason and known progress during a provider pause (%s, ${locale}, ${tokens} tokens)`, async (reason) => {
  const previous = currentLocale();
  await setLocale(locale);
  calls.overview.mockResolvedValue({...overview(), orgs: [org('paused-org', 'Paused org', {
    state: 'paused', reason, active, tokens, pending_tokens: tokens / 4,
    seconds_left: 5, tokens_per_second: 200,
  })]});
  const view = show(<SemanticSearch />);
  try {
    const row = await screen.findByRole('row', {name: /Paused org/});
    expect(row).toHaveTextContent(i18n.t('admin.semantic.states.paused'));
    const label = within(row).getByText(i18n.t(`admin.semantic.reasons.${reason}`));
    expect(label).toHaveClass('text-sm', 'text-text-muted');
    if (tokens > 0) expect(within(row).getByRole('meter')).toHaveAttribute('aria-valuenow', '75');
    else expect(within(row).queryByRole('meter')).toBeNull();
    const {locale: formatting, timeZone} = useAppearance.getState();
    expect(within(row).queryByText(i18n.t('admin.semantic.left', {time: formatSpan(5000, {locale: formatting, timeZone})}))).toBeNull();
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.queryByRole('button', {name: i18n.t('common.tryAgain')})).toBeNull();
  } finally { view.unmount(); await setLocale(previous); }
});

it.each(['en', 'zh-CN'] as const)('says where each org stands and what a waiting one may choose (%s)', async (locale) => {
  const previous = currentLocale();
  await setLocale(locale);
  calls.overview.mockResolvedValue(overview());
  const view = show(<SemanticSearch />);
  try {
    const indexing = await screen.findByRole('row', { name: /Acme/ });
    expect(indexing).toHaveTextContent(i18n.t('admin.semantic.states.indexing'));
    expect(indexing).toHaveTextContent('embeddinggemma-2 · 256');
    const { locale: formatting, timeZone } = useAppearance.getState();
    expect(indexing).toHaveTextContent(i18n.t('admin.semantic.left', { time: formatSpan(5_000, { locale: formatting, timeZone }) }));
    const waiting = screen.getByRole('row', { name: /Bureau/ });
    expect(waiting).toHaveTextContent(i18n.t('admin.semantic.states.waiting'));
    expect(waiting).toHaveTextContent(i18n.t('admin.semantic.reasons.choice'));
    expect(within(waiting).getByRole('button', { name: i18n.t('admin.semantic.rebuild') })).toBeEnabled();
    expect(within(waiting).getByRole('button', { name: i18n.t('admin.semantic.keep') })).toBeEnabled();
    const unnamed = screen.getByRole('row', { name: /Cafe/ });
    expect(within(unnamed).getByRole('button', { name: i18n.t('admin.semantic.addLocal') })).toBeEnabled();
    expect(within(indexing).queryByRole('button')).toBeNull();
    expect(screen.getByTestId('settings-editor')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).toBeNull();
  } finally {
    view.unmount();
    await setLocale(previous);
  }
});

it('keeps the exact index shown, including its model and chunk size, for that org', async () => {
  calls.overview.mockResolvedValue(overview());
  calls.keep.mockResolvedValue(org('0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a02', 'Bureau', {state: 'idle', active}));
  const view = show(<SemanticSearch />);
  try {
    const waiting = await screen.findByRole('row', { name: /Bureau/ });
    fireEvent.click(within(waiting).getByRole('button', { name: i18n.t('admin.semantic.keep') }));
    await waitFor(() => expect(calls.keep).toHaveBeenCalledOnce());
    expect(calls.keep).toHaveBeenCalledWith('0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a02', active.id);
    expect(calls.put).not.toHaveBeenCalled();
    expect(calls.rebuild).not.toHaveBeenCalled();
    await waitFor(() => expect(waiting).toHaveTextContent(i18n.t('admin.semantic.states.idle')));
  } finally {
    view.unmount();
  }
});

it('opens the ordinary provider dialog at the org with Local embeddings selected and does not write the search setting', async () => {
  calls.overview.mockResolvedValue(overview());
  const view = show(<SemanticSearch />);
  const unnamed = await screen.findByRole('row', {name: /Cafe/});
  fireEvent.click(within(unnamed).getByRole('button', {name: i18n.t('admin.semantic.addLocal')}));
  expect(await screen.findByRole('dialog', {name: 'Local embeddings'})).toHaveTextContent('0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a03');
  expect(calls.put).not.toHaveBeenCalled();
  view.unmount();
});

it('says why search by meaning is unavailable when the database cannot keep vectors', async () => {
  calls.overview.mockResolvedValue(overview(false));
  const view = show(<SemanticSearch />);
  try {
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(i18n.t('admin.semantic.unavailable'));
    expect(alert).toHaveTextContent(i18n.t('admin.semantic.absent'));
  } finally {
    view.unmount();
  }
});

it.each([400, 1440])('keeps the model, progress and disk readable once at %i px', async (width) => {
  const room = vi.spyOn(Element.prototype, 'clientWidth', 'get').mockReturnValue(width);
  calls.overview.mockResolvedValue(overview());
  const view = show(<SemanticSearch />);
  try {
    const indexing = await screen.findByRole('row', { name: /Acme/ });
    expect(within(indexing).getAllByText('embeddinggemma-2 · 256')).toHaveLength(1);
    expect(within(indexing).getAllByRole('meter')).toHaveLength(1);
    const { locale } = useAppearance.getState();
    expect(indexing).toHaveTextContent(i18n.t('admin.semantic.diskOf', { used: `~${formatBytes(1_000_000, locale)}`, allowed: formatBytes(2_000_000, locale) }));
  } finally {
    view.unmount();
    room.mockRestore();
  }
});

it.each(['en', 'zh-CN'] as const)('names measured disk even when the index has no limit in %s', async (locale) => {
  const previous = currentLocale();
  await setLocale(locale);
  calls.overview.mockResolvedValue({...overview(), orgs: [org('unlimited', 'Unlimited', {used_bytes: 1_000_000, allowed_bytes: null})]});
  const view = show(<SemanticSearch />);
  try {
    const row = await screen.findByRole('row', {name: /Unlimited/});
    const expected = i18n.t('admin.semantic.diskUsed', {used: `~${formatBytes(1_000_000, useAppearance.getState().locale)}`});
    expect(row).toHaveTextContent(expected);
    expect(expected).toMatch(locale === 'en' ? /^Disk ~/ : /^磁盘 ~/);
  } finally {view.unmount(); await setLocale(previous);}
});

it('shows a stalled index with its reason, and an empty installation without a warning', async () => {
  calls.overview.mockResolvedValue({ ...overview(), orgs: [org('org-paused', 'Stalled', { state: 'paused', reason: 'disk_share' })] });
  const view = show(<SemanticSearch />);
  const paused = await screen.findByRole('row', { name: /Stalled/ });
  expect(paused).toHaveTextContent(i18n.t('admin.semantic.states.paused'));
  expect(paused).toHaveTextContent(`· ${i18n.t('admin.semantic.reasons.disk_share')}`);
  expect(screen.queryByRole('alert')).toBeNull();
  view.unmount();
  calls.overview.mockResolvedValue({ ...overview(), orgs: [] });
  show(<SemanticSearch />);
  expect(await screen.findByText(i18n.t('admin.semantic.noOrgs'))).toBeVisible();
  expect(screen.queryByRole('table')).toBeNull();
});

it('keeps a pending read in the loading state and gives a failed read its retry', async () => {
  calls.overview.mockReturnValue(new Promise(() => undefined));
  const loading = show(<SemanticSearch />);
  expect(screen.getByRole('status')).toHaveAttribute('aria-busy', 'true');
  expect(screen.queryByRole('button')).toBeNull();
  loading.unmount();
  calls.overview.mockRejectedValue(new Error('Offline'));
  show(<SemanticSearch />);
  expect(await screen.findByRole('button', { name: i18n.t('common.tryAgain') })).toBeEnabled();
});

it('counts the inventory without a false completed meter or whole-index ETA',async()=>{
  const rows=overview();rows.orgs=[org('counting','Acme',{state:'indexing',reason:'counting',tokens:4000,pending_tokens:0,seconds_left:null})];
  calls.overview.mockResolvedValue(rows);const view=show(<SemanticSearch/>);
  const row=await screen.findByRole('row',{name:/Acme/});
  expect(row).toHaveTextContent(i18n.t('admin.semantic.counting',{tokens:formatTokens(4000,useAppearance.getState().locale)}));
  expect(row).not.toHaveTextContent(i18n.t('admin.semantic.done'));
  expect(within(row).queryByRole('meter')).not.toBeInTheDocument();view.unmount();
});

it.each(['en','zh-CN'] as const)('links the used ordinary template to its editor in %s',async(locale)=>{
  const previous=currentLocale();await setLocale(locale);calls.templateId='template-local';
  calls.overview.mockResolvedValue({...overview(),orgs:[org('template-org','Acme',{})]});
  const root=createRootRoute({component:SemanticSearch});
  const template=createRoute({getParentRoute:()=>root,path:'/resources/templates/$id',component:()=>null});
  const router=createRouter({routeTree:root.addChildren([template]),history:createMemoryHistory({initialEntries:['/']})});
  const view=show(<RouterProvider router={router}/>);
  try {
    const row=await screen.findByRole('row',{name:/Acme/});
    const link=await within(row).findByRole('link',{name:i18n.t('admin.semantic.template',{name:'Local embeddings'})});
    expect(link).toHaveAttribute('href','/resources/templates/template-local');
    fireEvent.click(link);
    await waitFor(()=>expect(router.state.location.pathname).toBe('/resources/templates/template-local'));
  } finally {view.unmount();await setLocale(previous);}
});
