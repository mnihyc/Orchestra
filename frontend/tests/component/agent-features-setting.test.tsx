/** The agent features: a named switch for each feature that a parameter of an installed program carries, with what it opens; a stored feature nobody names is shown with a way to remove it. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import type * as ResourcesApi from '@/api/resources';
import { AgentFeaturesChoice, AgentFeaturesSummary, featureOffers, firstSentence } from '@/features/settings/AgentFeaturesChoice';
import type { AgentDescriptor, HarnessKindInfo } from '@/generated/api-types';
import { agentDescriptor } from '../doubles/agent-descriptor';
import { harnessName } from '../doubles/harness-names';
import { show } from './support';

const state = vi.hoisted(() => ({ kinds: [] as HarnessKindInfo[] }));
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof ResourcesApi>(), harnessKindsQuery: { queryKey: ['agent-features-kinds'], queryFn: () => Promise.resolve(state.kinds) } }));

const ACCESS = 'An authorised program for security work, which the vendor grants to the account. Only the programs a manager of the provider enabled for the model are offered.';
const parameter = (key: string, title: string, description: string, feature?: string) => ({
  key, title, description, group: 'safety', type: 'enum', options: { from_model: 'access_programs' }, default: null, scopes: ['thread'], apply: 'next_turn', ...(feature === undefined ? {} : { feature }),
});
function kind(name: string, parameters: unknown[]): HarnessKindInfo {
  const harness = harnessName(name);
  return { kind: name, title: harness.name, builds: [], program: null, harness, descriptor: agentDescriptor(harness.name, { kind: name, parameters: parameters as AgentDescriptor['parameters'] }) };
}
const codex = () => { const declared = kind('codex', [parameter('cyber_access_program', 'Access program', ACCESS, 'codex.cyber_access'), parameter('reasoning_summary', 'Reasoning summary', 'How much it says.')]); declared.descriptor = Object.assign({}, declared.descriptor, { features: { 'codex.cyber_access': true } }); return declared; };

function Draft({ initial = {}, inherited = {}, changed }: { initial?: unknown; inherited?: unknown; changed?: (value: unknown) => void }) {
  const [value, setValue] = useState<unknown>(initial);
  return <AgentFeaturesChoice shape={{ type: 'json' }} label="Agent features" value={value} inherited={inherited}
    onChange={(next) => { setValue(next); changed?.(next); }} />;
}

it('says the first sentence of what a parameter does, whole', () => {
  expect(firstSentence(ACCESS)).toBe('An authorised program for security work, which the vendor grants to the account.');
  expect(firstSentence('Faster')).toBe('Faster');
  expect(firstSentence('')).toBe('');
});

it('names a feature by the program and the parameter that carry it, one entry for a feature however many name it', () => {
  const shared = kind('claude-code', [parameter('program', 'Access program', 'Another program.', 'codex.cyber_access'), parameter('plan', 'Plan', 'Plans first.', 'plans')]);
  shared.descriptor = Object.assign({}, shared.descriptor, { features: { 'codex.cyber_access': true } });
  const offers = featureOffers([codex(), shared], 'en', (name) => harnessName(name).name);
  // In the order of their names.
  expect(offers.map((offer) => offer.feature)).toEqual(['plans', 'codex.cyber_access']);
  expect(offers[0]?.name).toBe('Claude Code: Plan');
  expect(offers[1]?.name).toBe('Codex, Claude Code: Access program');
});

it('shows a switch for each feature a parameter carries, with the parameter\'s title and what it opens, and no key or JSON', async () => {
  state.kinds = [codex(), kind('claude-code', [])];
  show(<Draft />);
  const access = await screen.findByRole('switch', { name: 'Codex: Access program' });
  expect(screen.getAllByRole('switch')).toHaveLength(1);
  expect(access).toBeChecked();
  expect(screen.getByText('An authorised program for security work, which the vendor grants to the account.')).toBeInTheDocument();
  expect(screen.queryByText(/codex\.cyber_access/u)).not.toBeInTheDocument();
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
});

it('writes only what differs from the default when a feature is switched, and takes the entry out when it is switched back', async () => {
  const user = userEvent.setup();
  const changed = vi.fn();
  state.kinds = [codex()];
  show(<Draft changed={changed} />);
  await user.click(await screen.findByRole('switch', { name: 'Codex: Access program' }));
  expect(changed).toHaveBeenLastCalledWith({ 'codex.cyber_access': false });
  expect(screen.getByRole('switch', { name: 'Codex: Access program' })).not.toBeChecked();
  await user.click(screen.getByRole('switch', { name: 'Codex: Access program' }));
  expect(changed).toHaveBeenLastCalledWith({});
});

it('follows what places above set until the place says otherwise, and offers going back to it', async () => {
  const user = userEvent.setup();
  const changed = vi.fn();
  state.kinds = [codex()];
  show(<Draft inherited={{ 'codex.cyber_access': false }} changed={changed} />);
  const access = await screen.findByRole('switch', { name: 'Codex: Access program' });
  expect(access).not.toBeChecked();
  expect(screen.queryByRole('button', { name: /Use the default/u })).not.toBeInTheDocument();
  await user.click(access);
  expect(changed).toHaveBeenLastCalledWith({ 'codex.cyber_access': true });
  await user.click(screen.getByRole('button', { name: 'Use the default for Codex: Access program' }));
  expect(changed).toHaveBeenLastCalledWith({});
  expect(screen.getByRole('switch', { name: 'Codex: Access program' })).not.toBeChecked();
});

it('shows a stored feature that no installed program names, with a way to remove it, and keeps the others', async () => {
  const user = userEvent.setup();
  const changed = vi.fn();
  state.kinds = [codex()];
  show(<Draft initial={{ 'future.thing': true, 'codex.cyber_access': false }} changed={changed} />);
  expect(await screen.findByText('future.thing')).toBeInTheDocument();
  expect(screen.getByText(/No installed agent program names this feature/u)).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Remove future.thing' }));
  expect(changed).toHaveBeenLastCalledWith({ 'codex.cyber_access': false });
  expect(screen.queryByText('future.thing')).not.toBeInTheDocument();
});

it('says so where no installed program has a feature to switch', async () => {
  state.kinds = [kind('claude-code', [parameter('plan', 'Plan', 'Plans first.')])];
  show(<Draft />);
  expect(await screen.findByText('No installed agent program has a feature to switch.')).toBeInTheDocument();
  expect(screen.queryByRole('switch')).not.toBeInTheDocument();
});

it('summarises the value by the program and the parameter, and says where nothing is set', async () => {
  state.kinds = [codex()];
  const { unmount } = show(<AgentFeaturesSummary value={{ 'codex.cyber_access': true }} />);
  expect(await screen.findByText('Codex: Access program (On)')).toBeInTheDocument();
  unmount();
  show(<AgentFeaturesSummary value={{}} />);
  expect(screen.getByText('Program default')).toBeInTheDocument();
});

it('reads a second program’s shipped feature and its source from the declaration with empty shared defaults', async () => {
  const second = kind('fake', [parameter('tools', 'Extra tools', 'Additional tools.', 'fake.extra_controls')]);
  second.descriptor = Object.assign({}, second.descriptor, { features: { 'fake.extra_controls': true } });
  state.kinds = [second];
  const changed = vi.fn();
  show(<Draft changed={changed} />);
  const feature = await screen.findByRole('switch', { name: /Extra tools/ });
  expect(feature).toBeChecked();
  expect(screen.getByText('Program default')).toBeVisible();
  await userEvent.click(feature);
  expect(changed).toHaveBeenLastCalledWith({ 'fake.extra_controls': false });
});
