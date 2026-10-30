/** A provider's model and group actions open and submit the application's real limit dialog. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import type * as ModelsApi from '@/api/models';
import type * as ResourcesApi from '@/api/resources';
import { providerKey } from '@/api/resources';
import type * as UsageApi from '@/api/usage';
import type { Candidate, Candidates } from '@/features/access/candidates';
import { ModelLimits } from '@/features/resources/providers/Caps';
import { GroupStrip } from '@/features/resources/providers/Groups';
import { CapHost } from '@/features/usage/AddCap';
import type { CapInput, ProviderModel, UsageCap } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';
import { MODEL_ESTIMATE } from '../doubles/fixtures';

const calls = vi.hoisted(() => ({ create: vi.fn<typeof UsageApi.createCap>() }));
vi.mock('@/api/usage', async (original) => ({ ...await original<typeof UsageApi>(), createCap: calls.create }));
vi.mock('@/api/models', async (original) => ({ ...await original<typeof ModelsApi>(),
  providerCapsQuery: (id: string) => ({ queryKey: ['provider-caps', id], queryFn: () => Promise.resolve([]) }),
}));
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof ResourcesApi>(),
  providerQuery: (id: string) => ({ queryKey: ['provider', id], queryFn: () => Promise.resolve(provider) }),
  harnessKindsQuery: { queryKey: ['limit-entry-harnesses'], queryFn: () => Promise.resolve([]) },
}));
// Holder searching has its own coverage; this form starts with everyone, not a selected holder.
vi.mock('@/features/access/candidates', () => ({ useCandidates: (): Candidates => ({ groups: [], byValue: new Map<string, Candidate>(), loading: false, placePicker: null }) }));

const model: ProviderModel = { ...MODEL_ESTIMATE, id: 'model-pro', model_name: 'fable-pro', display_name: 'Fable Pro', enabled: true, source: 'manual', groups: ['frontier'], parameters: {} };
const provider = planProvider({ models: [model] });
type Entry = 'model' | 'group';
const pattern = (entry: Entry): string => entry === 'model' ? model.model_name : '@frontier';

function bodyFor(entry: Entry): CapInput {
  return { provider_id: provider.id, subject: { kind: 'everyone' }, split: 'shared', models: [pattern(entry)], tiers: [], harness_kinds: [],
    meter: 'tokens', amount: '2000000', period: 'day', at_limit: 'stop', uncounted: 'allow', warn_percent: 80 };
}

function made(entry: Entry): UsageCap {
  return { id: 'created-provider-limit', node: null, provider, subject: { kind: 'everyone' }, split: 'shared', models: [pattern(entry)], tiers: [], harness_kinds: [],
    meter: 'tokens', amount: '2000000', period: 'day', counted_from: null, at_limit: 'stop', uncounted: 'allow', warn_percent: 80,
    note: '', enabled: true, revision: 1, created_by: { id: 'limit-maker', kind: 'human', username: 'dana', display_name: 'Dana', avatar: { text: 'D', color: 1 } },
    created_at: '2026-10-14T12:00:00Z', updated_at: '2026-10-14T12:00:00Z', can_edit: true };
}

// Transform the real lazy form as fixture setup; each case keeps the real host and request flow.
beforeAll(async () => { await import('@/features/usage/CapDialog'); });

let client: QueryClient;
let locale = currentLocale();
let appearance = useAppearance.getState();
beforeEach(() => {
  locale = currentLocale(); appearance = useAppearance.getState(); calls.create.mockReset();
  client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
});
afterEach(async () => {
  if (screen.queryByRole('dialog') !== null) await userEvent.keyboard('{Escape}');
  // The real host releases the closed form after its exit; no second host or test-only reset API.
  await waitFor(() => expect(client.getQueryCache().find({ queryKey: providerKey(provider.id) })?.getObserversCount() ?? 0).toBe(0));
  cleanup(); client.clear(); useAppearance.setState(appearance); await setLocale(locale);
});

function draw(entry: Entry, manage = true) {
  show(<QueryClientProvider client={client}>
    {entry === 'model' ? <ModelLimits provider={provider} model={model} /> : <GroupStrip provider={provider} models={provider.models} manage={manage} />}
    <CapHost />
  </QueryClientProvider>);
}

async function open(entry: Entry) {
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: i18n.t(entry === 'model' ? 'models.limits.add' : 'models.groups.limit') }));
  await screen.findByRole('textbox', { name: i18n.t('caps.form.amount') });
  return { user, dialog: screen.getByRole('dialog', { name: i18n.t('caps.form.title') }) };
}

describe.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('provider limit entries in %s/%s', (language, theme) => {
  beforeEach(async () => { await setLocale(language); useAppearance.setState({ locale: language, theme, resolvedTheme: theme }); });

  it.each(['model', 'group'] as const)('prefills the %s and provider, then submits one native limit and closes on confirmation', async (entry) => {
    calls.create.mockResolvedValue(made(entry));
    draw(entry);
    const { user, dialog } = await open(entry);
    expect(screen.getAllByRole('dialog')).toHaveLength(1);
    expect(within(dialog).getByText(i18n.t('caps.where.provider', { name: provider.name }), { exact: true })).toBeVisible();
    expect(within(dialog).getByRole('list', { name: i18n.t('caps.form.modelsChosen') })).toHaveTextContent(entry === 'model' ? model.model_name : i18n.t('caps.on.group', { name: 'frontier' }));
    expect(within(dialog).getByRole('combobox', { name: i18n.t('caps.form.who') })).toHaveTextContent(i18n.t('caps.form.whoChoices.everyone'));
    await user.type(within(dialog).getByRole('textbox', { name: i18n.t('caps.form.amount') }), '2M');
    await user.click(within(dialog).getByRole('button', { name: i18n.t('caps.form.make') }));
    await waitFor(() => expect(calls.create).toHaveBeenCalledExactlyOnceWith(bodyFor(entry), expect.any(String)));
    expect(calls.create.mock.calls[0]?.[1]).not.toBe('');
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  });

  it('keeps the exact provider, group and draft after a refused write and retries the same request', async () => {
    calls.create.mockRejectedValueOnce(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Access denied', detail: 'Managing this provider requires permission.', status: 403, request_id: 'limit-write-refused' }, null));
    calls.create.mockResolvedValue(made('group'));
    draw('group');
    const { user, dialog } = await open('group');
    const amount = within(dialog).getByRole('textbox', { name: i18n.t('caps.form.amount') });
    await user.type(amount, '2M');
    await user.click(within(dialog).getByRole('button', { name: i18n.t('caps.form.make') }));
    expect(await within(dialog).findByRole('alert')).toHaveTextContent(i18n.t('errors.authz.denied'));
    expect(amount).toHaveValue('2M');
    expect(within(dialog).getByRole('list', { name: i18n.t('caps.form.modelsChosen') })).toHaveTextContent(i18n.t('caps.on.group', { name: 'frontier' }));
    const first = calls.create.mock.calls[0];
    expect(first).toEqual([bodyFor('group'), expect.any(String)]);
    await user.click(within(dialog).getByRole('button', { name: i18n.t('caps.form.make') }));
    await waitFor(() => expect(calls.create).toHaveBeenCalledTimes(2));
    expect(calls.create.mock.calls[1]).toEqual(first);
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  });

  it('keeps the groups readable without offering a creation action to a read-only viewer', () => {
    draw('group', false);
    expect(screen.getByText('frontier', { exact: true })).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('models.groups.limit') })).not.toBeInTheDocument();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(calls.create).not.toHaveBeenCalled();
  });
});
