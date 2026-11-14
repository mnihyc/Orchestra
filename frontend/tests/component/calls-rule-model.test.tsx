/** The model a call rule matches is chosen among the models of the provider the rule names; without a provider it stays a name that is typed. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import type * as ResourcesApi from '@/api/resources';
import { CallsRules } from '@/features/settings/CallsRules';
import type { ProviderModel } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { show } from './support';

const models = [{ id: 'm1', model_name: 'fast-1', display_name: 'Fast model', enabled: true }, { id: 'm2', model_name: 'slow-1', display_name: 'Slow model', enabled: false }] as unknown as ProviderModel[];
vi.mock('@/features/settings/ProviderChoice', () => ({ ProviderDefaultChoice: ({ label }: { label: string }) => <span>{label}</span> }));
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof ResourcesApi>(),
  providerModelsQuery: (id: string) => ({ queryKey: ['rule-model-models', id], queryFn: () => Promise.resolve(models) }),
  providerQuery: (id: string) => ({ queryKey: ['rule-model-provider', id], queryFn: () => Promise.resolve({ id, kind: 'api', protocols: ['openai-responses'], agent: null, agent_id: null, harness_kind: null }) }),
  agentsQuery: () => ({ queryKey: ['rule-model-agents'], queryFn: () => Promise.resolve([]) }) }));
vi.mock('@/features/change/target', () => ({ openChange: vi.fn() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

const rule = (when: object) => ({ id: 'refused', when: { kind: 'model_refused', ...when }, steps: [{ type: 'wait', until: 'person' }] });
const draw = (when: object, change = vi.fn()) => show(<CallsRules shape={{ type: 'json' }} value={[rule(when)]} label={i18n.t('calls.rules')} onChange={change} />);

it('chooses the model among the provider\'s enabled models, and writes its routing name', async () => {
  const user = userEvent.setup();
  const change = vi.fn();
  draw({ provider_id: 'provider-1' }, change);
  await user.click(screen.getByRole('button', { name: /→/ }));
  const dialog = screen.getByRole('dialog', { name: i18n.t('calls.editRule') });
  await user.click(within(dialog).getByRole('combobox', { name: i18n.t('calls.model') }));
  expect(await screen.findByRole('option', { name: 'Fast model' })).toBeVisible();
  expect(screen.queryByRole('option', { name: 'Slow model' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('option', { name: 'Fast model' }));
  await user.click(within(dialog).getByRole('button', { name: i18n.t('common.save') }));
  expect(change.mock.calls[0]?.[0]).toEqual([rule({ provider_id: 'provider-1', model: 'fast-1' })]);
});

it('goes back to any model with one press', async () => {
  const user = userEvent.setup();
  const change = vi.fn();
  draw({ provider_id: 'provider-1', model: 'fast-1' }, change);
  await user.click(screen.getByRole('button', { name: /→/ }));
  const dialog = screen.getByRole('dialog', { name: i18n.t('calls.editRule') });
  await user.click(within(dialog).getByRole('button', { name: i18n.t('calls.anyModel') }));
  await user.click(within(dialog).getByRole('button', { name: i18n.t('common.save') }));
  expect(change.mock.calls[0]?.[0]).toEqual([rule({ provider_id: 'provider-1' })]);
});

it('keeps a model name typed where the rule names no provider', async () => {
  const user = userEvent.setup();
  draw({});
  await user.click(screen.getByRole('button', { name: /→/ }));
  const dialog = screen.getByRole('dialog', { name: i18n.t('calls.editRule') });
  expect(within(dialog).getByRole('textbox', { name: i18n.t('calls.model') })).toBeVisible();
  expect(within(dialog).queryByRole('combobox', { name: i18n.t('calls.model') })).not.toBeInTheDocument();
});
