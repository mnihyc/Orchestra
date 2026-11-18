/** The model of a place is chosen by name from the models of the provider in force there; a name the provider does not list can still be typed. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import type * as ResourcesApi from '@/api/resources';
import type * as SettingsApi from '@/api/settings';
import { AgentModelChoice } from '@/features/settings/AgentModelChoice';
import type { ProviderModel } from '@/generated/api-types';
import { show } from './support';

const state = vi.hoisted(() => ({ rows: [] as { key: string; value: unknown }[], models: [] as unknown[] }));
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof SettingsApi>(), targetSettingsQuery: () => ({ queryKey: ['agent-model-place'], queryFn: () => Promise.resolve(state.rows) }) }));
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof ResourcesApi>(), providerModelsQuery: (id: string) => ({ queryKey: ['agent-model-models', id], queryFn: () => Promise.resolve(state.models) }) }));

const target = { kind: 'node', id: 'org' } as const;
const models = [{ id: 'm1', model_name: 'fast-1', display_name: 'Fast model', enabled: true }, { id: 'm2', model_name: 'slow-1', display_name: 'Slow model', enabled: false }] as unknown as ProviderModel[];

function draw(value: string, changed = vi.fn()) {
  show(<AgentModelChoice shape={{ type: 'text', maxLength: null, pattern: null }} label="Model" value={value} target={target} onChange={changed} />);
  return changed;
}

beforeEach(() => {
  state.rows = [{ key: 'agent.default_provider_id', value: 'provider-1' }];
  state.models = models;
});

it('offers the enabled models of the provider in force, by name, and writes the one chosen', async () => {
  const user = userEvent.setup();
  const changed = draw('');
  await user.click(screen.getByRole('combobox', { name: 'Model' }));
  expect(await screen.findByRole('option', { name: 'Fast model' })).toBeVisible();
  expect(screen.queryByRole('option', { name: 'Slow model' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('option', { name: 'Fast model' }));
  expect(changed).toHaveBeenCalledWith('fast-1');
});

it('keeps a name the provider does not list possible, by typing it', async () => {
  const user = userEvent.setup();
  const changed = draw('');
  await user.click(screen.getByRole('combobox', { name: 'Model' }));
  await user.type(screen.getByRole('combobox', { name: 'Model' }), 'brand-new');
  await user.click(await screen.findByRole('option', { name: 'Use “brand-new”' }));
  expect(changed).toHaveBeenCalledWith('brand-new');
});

it('says beside the field that the provider does not offer the model that is saved', async () => {
  draw('gone');
  expect(await screen.findByText('This provider does not offer gone.')).toBeVisible();
});

it('takes a typed name where no provider is in force, and asks for no identifier', async () => {
  const user = userEvent.setup();
  state.rows = [{ key: 'agent.default_provider_id', value: null }];
  const changed = draw('');
  const field = screen.getByRole('combobox', { name: 'Model' });
  expect(field).toBeEnabled();
  expect(field).toHaveAttribute('placeholder', 'Type a model name');
  await user.type(field, 'some-model');
  await user.click(await screen.findByRole('option', { name: 'Use “some-model”' }));
  expect(changed).toHaveBeenCalledWith('some-model');
});
