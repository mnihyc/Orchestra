/** The provider is the model's, and the agent has a list of its own: ten providers are ten headings with their models, four agents are four rows, and a provider that has to be asked for opens the request. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';

import { type AgentChoice, AgentChip, agentAfterProvider, agentChangedWords, agentChoicesOf, type ProviderChoice, providerAccess } from '@/features/composer/BindingLists';
import { ProviderModelChip, type ProviderModels } from '@/features/composer/ProviderModelChip';
import type { BindingOptions, ModelOption, UserRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { MODEL_ESTIMATE } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@/features/access/AccessRequest', () => ({
  AccessRequest: ({ objectId, missing }: { objectId: string; missing: readonly { permissions: string[] }[] }) => <div role="group" aria-label="request" data-object={objectId} data-permissions={missing.flatMap((entry) => entry.permissions).join()} />,
}));
afterEach(async () => { cleanup(); await setLocale('en'); });

const owner = { id: 'owner-1', display_name: 'Owen Owner' } as unknown as UserRef;
const choice = (id: string, name: string, change: Partial<ProviderChoice> = {}): ProviderChoice => ({ id, name, address: `acme.${id}`, status: 'ready', usable: true, reason: null, owner, ...change });
const model = (name: string): ModelOption => ({ ...MODEL_ESTIMATE, model_name: `${name}-routing`, display_name: name, allowed: true, reason: null, parameters: { context_window: 200_000 } });
const chip = (providers: ProviderModels[], onModel = vi.fn()) => show(<ProviderModelChip what={i18n.t('composer.model.chip')} searchLabel={i18n.t('composer.model.find')} providers={providers} open
  current={{ provider: providers[0]?.provider.id ?? '', model: 'Model 0-routing' }} name="Model 0" onModel={onModel} />);

it('groups ten providers under their names, a model to a row, with search', async () => {
  const providers = Array.from({ length: 10 }, (_, at) => ({ provider: choice(`p${at}`, `Provider ${at}`), models: [model(`Model ${at}`)] }));
  chip(providers);
  const menu = await screen.findByRole('menu', { name: i18n.t('composer.model.chip') });
  expect(within(menu).getAllByRole('menuitemradio')).toHaveLength(10);
  expect(within(menu).getAllByText(/^Provider \d$/)).toHaveLength(10);
  expect(screen.getByRole('textbox', { name: i18n.t('composer.model.find') })).toBeVisible();
  // The model of the provider the thread is on is the one that is checked.
  expect(within(menu).getByRole('menuitemradio', { name: 'Model 0', checked: true })).toBeVisible();
});

it('lists the providers to ask for and the unavailable ones last, and opens the request for the one that is asked for', async () => {
  const user = userEvent.setup(), onModel = vi.fn();
  chip([
    { provider: choice('p0', 'Team key'), models: [model('Model 0')] },
    { provider: choice('shared', 'Shared key', { usable: false, reason: 'no_use' }), models: [model('Model 1')] },
    { provider: choice('broken', 'Broken key', { status: 'needs_setup' }), models: [model('Model 2')] },
    { provider: choice('stuck', 'Stuck key', { usable: false, reason: 'no_use', owner: null }), models: [] },
  ], onModel);
  const menu = await screen.findByRole('menu', { name: i18n.t('composer.model.chip') });
  const names = within(menu).getAllByRole('menuitemradio').map((row) => row.textContent);
  expect(names[0]).toContain('Model 0');
  expect(within(menu).getByText(i18n.t('composer.pick.provider.ask'))).toBeVisible();
  expect(within(menu).getByText(i18n.t('composer.pick.provider.off'))).toBeVisible();
  // Unavailable rows are greyed with the reason in a few words; their models are not offered.
  const broken = within(menu).getByRole('menuitemradio', { name: /^Broken key/ });
  expect(broken).toBeDisabled(); expect(broken.closest('li')).toHaveTextContent(i18n.t('composer.pick.provider.why.needs_setup'));
  expect(within(menu).getByRole('menuitemradio', { name: /^Stuck key/ })).toBeDisabled();
  expect(within(menu).queryByRole('menuitemradio', { name: 'Model 2' })).not.toBeInTheDocument();
  // Choosing a provider that is asked for opens the request for its use, and chooses nothing.
  await user.click(within(menu).getByRole('menuitemradio', { name: 'Shared key' }));
  const request = await screen.findByRole('group', { name: 'request' });
  expect(request).toHaveAttribute('data-object', 'shared'); expect(request).toHaveAttribute('data-permissions', 'provider.use');
  expect(onModel).not.toHaveBeenCalled();
});

it('chooses the model of another provider and hands back both', async () => {
  const user = userEvent.setup(), onModel = vi.fn();
  const providers = [{ provider: choice('p0', 'Team key'), models: [model('Model 0')] }, { provider: choice('p1', 'Other key'), models: [model('Model 1')] }];
  chip(providers, onModel);
  await user.click(await screen.findByRole('menuitemradio', { name: 'Model 1' }));
  expect(onModel).toHaveBeenCalledWith(providers[1]?.provider, providers[1]?.models[0], expect.any(Function));
});

const agent = (id: string, name: string, change: Partial<AgentChoice> = {}): AgentChoice => ({ id, name, fits: true, usable: true, reason: null, ...change });

it('lists the four agents that work with the provider and nothing more', async () => {
  const choices = ['Claude Code', 'Codex', 'DeepSeek', 'Example agent'].map((name, at) => agent(`a${at}`, name));
  show(<AgentChip choices={choices} value="a1" name="Codex" provider="OpenAI third" onChoose={vi.fn()} open />);
  const menu = await screen.findByRole('menu', { name: i18n.t('composer.row.agentName') });
  expect(within(menu).getAllByRole('menuitemradio')).toHaveLength(4);
  expect(within(menu).getByRole('menuitemradio', { name: 'Codex', checked: true })).toBeVisible();
  expect(screen.queryByText(i18n.t('composer.pick.agent.off', { provider: 'OpenAI third' }))).not.toBeInTheDocument();
});

it('greys the agents that do not work with the provider under a header that says so', async () => {
  const choices = [agent('a0', 'Codex'), agent('a1', 'Claude Code', { fits: false, usable: false }), agent('a2', 'Slow agent', { usable: false, reason: 'harness_unreported' })];
  show(<AgentChip choices={choices} value="a0" name="Codex" provider="OpenAI third" onChoose={vi.fn()} open />);
  const menu = await screen.findByRole('menu', { name: i18n.t('composer.row.agentName') });
  expect(within(menu).getByText(i18n.t('composer.pick.agent.off', { provider: 'OpenAI third' }))).toBeVisible();
  expect(within(menu).getByRole('menuitemradio', { name: 'Claude Code' })).toBeDisabled();
  const slow = within(menu).getByRole('menuitemradio', { name: /^Slow agent/ });
  expect(slow).toBeDisabled(); expect(slow.closest('li')).toHaveTextContent(i18n.t('composer.pick.agent.why.unreported'));
});

it('says why an agent that does not work with the provider is off where that is known, and says nothing where it is not', async () => {
  const choices = [agent('a0', 'Codex'), agent('a3', 'Claude Code'), agent('a1', 'Earlier agent', { fits: false, usable: false, reason: 'disabled' }), agent('a2', 'Other agent', { fits: false, usable: false })];
  show(<AgentChip choices={choices} value="a0" name="Codex" provider="OpenAI third" onChoose={vi.fn()} open />);
  const menu = await screen.findByRole('menu', { name: i18n.t('composer.row.agentName') });
  const earlier = within(menu).getByRole('menuitemradio', { name: /^Earlier agent/ });
  expect(earlier).toBeDisabled(); expect(earlier.closest('li')).toHaveTextContent(i18n.t('composer.pick.agent.why.disabled'));
  expect(within(menu).getByRole('menuitemradio', { name: 'Other agent' }).closest('li')).not.toHaveTextContent(i18n.t('composer.pick.agent.why.disabled'));
});

it('shows a provider that runs one agent only as that agent, chosen and not to be changed', () => {
  show(<AgentChip choices={[agent('a0', 'Codex'), agent('a1', 'Claude Code', { fits: false, usable: false })]} value="a0" name="Codex" provider="Plan login" onChoose={vi.fn()} />);
  const button = screen.getByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.row.agentName'), value: 'Codex' }) });
  expect(button).toBeDisabled();
});

it('changes the agent with the provider when the agent does not work with it, and says so in one line', () => {
  const options = { providers: [
    { provider: { id: 'p1', name: 'OpenAI third', address: 'a', status: 'ready' }, usable: true, reason: null, owner, models: [], agents: [{ agent: { id: 'codex', name: 'Codex' }, usable: true, reason: null }] },
    { provider: { id: 'p2', name: 'Claude plan', address: 'b', status: 'ready' }, usable: true, reason: null, owner, models: [], agents: [{ agent: { id: 'claude', name: 'Claude Code' }, usable: true, reason: null }, { agent: { id: 'codex', name: 'Codex' }, usable: true, reason: null }] },
  ] } as unknown as BindingOptions;
  // Codex works with both: it is kept. Claude Code works with the second only: going to the first changes the agent.
  expect(agentAfterProvider(agentChoicesOf(options, 'p2'), 'codex')).toMatchObject({ changed: false, agent: { id: 'codex' } });
  const moved = agentAfterProvider(agentChoicesOf(options, 'p1'), 'claude');
  expect(moved).toMatchObject({ changed: true, agent: { id: 'codex' } });
  expect(agentChoicesOf(options, 'p1').find((one) => one.id === 'claude')).toMatchObject({ fits: false });
  if (moved === null) throw new Error('Codex works with the first provider');
  expect(agentChangedWords(moved.agent, 'Claude Code', 'OpenAI third', i18n.t)).toBe(i18n.t('composer.pick.agent.changed', { agent: 'Codex', was: 'Claude Code', provider: 'OpenAI third' }));
  expect(i18n.t('composer.pick.agent.changed', { agent: 'Codex', was: 'Claude Code', provider: 'OpenAI third' })).toBe('Agent changed to Codex: Claude Code does not work with OpenAI third');
});

it('sorts a provider into what the person can use, ask for, or cannot have', () => {
  expect(providerAccess(choice('a', 'A'))).toBe('use');
  expect(providerAccess(choice('a', 'A', { status: 'exhausted' }))).toBe('use');
  expect(providerAccess(choice('a', 'A', { usable: false, reason: 'no_use' }))).toBe('ask');
  expect(providerAccess(choice('a', 'A', { usable: false, reason: 'no_use', owner: null }))).toBe('off');
  expect(providerAccess(choice('a', 'A', { status: 'error' }))).toBe('off');
  expect(providerAccess(choice('a', 'A', { usable: false, reason: 'provider_not_allowed' }))).toBe('off');
});
