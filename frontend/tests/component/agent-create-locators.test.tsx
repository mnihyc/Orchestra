/** Complete machine/adapter projections remain searchable without creating a profile implicitly. */
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import type * as Resources from '@/api/resources';
import { AddAgentDialog } from '@/features/resources/agents/AddAgent';
import type { HomePickerProps } from '@/features/resources/HomePicker';
import type { HarnessKindInfo, Machine } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { id } from '../doubles/fixtures';
import { detailAgent, detailMachine } from './resource-detail-fixtures';
import { show } from './support';

import { harnessName, partialDescriptor } from '../doubles/harness-names';
const calls = vi.hoisted(() => ({ registry: vi.fn<() => Promise<HarnessKindInfo[]>>(), machines: vi.fn<() => Promise<Machine[]>>(),
  machine: vi.fn<(id: string) => Promise<Machine>>(), create: vi.fn<typeof Resources.createAgent>(), register: vi.fn<typeof Resources.registerAgent>() }));
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof Resources>(), createAgent: calls.create, registerAgent: calls.register,
  harnessKindsQuery: { queryKey: ['create-locator-kinds'], queryFn: calls.registry },
  machinesQuery: { queryKey: ['create-locator-machines'], queryFn: calls.machines },
  machineQuery: (key: string) => ({ queryKey: ['create-locator-machine', key], queryFn: () => calls.machine(key) }) }));
vi.mock('@tanstack/react-router', () => ({ useNavigate: () => vi.fn() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/features/resources/HomePicker', () => ({ HomePicker: ({ value, onChange }: Pick<HomePickerProps, 'value' | 'onChange'>) =>
  <select aria-label="Fixture home" value={value} onChange={(event) => onChange(event.target.value)}>
    <option value="">Choose</option><option value="home">Workshop</option>
  </select> }));

const registry: HarnessKindInfo[] = Array.from({ length: 13 }, (_, index) => ({ kind: `adapter-${index + 1}`,
  title: `Registered adapter ${index + 1}`, builds: [], descriptor: partialDescriptor({ harness: { name: `Registered adapter ${index + 1}` } }),
  harness: harnessName(`adapter-${index + 1}`), program: null }));
const machines: Machine[] = Array.from({ length: 13 }, (_, index) => ({ ...detailMachine, id: id(90, index + 1),
  name: `Connected computer ${index + 1}`, roles: ['agent'], online: index !== 0,
  facts: { adapters: registry.map((entry) => ({ kind: entry.kind, needs_binary: false, builtin: false })), harnesses: [] } }));
const excluded: Machine = { ...detailMachine, id: id(90, 14), name: 'Environment only', roles: ['environment'] };
const field = (name: string) => screen.getByRole('combobox', { name });
const list = (name: string) => within(screen.getByRole('listbox', { name }));

beforeEach(async () => {
  await setLocale('en'); vi.clearAllMocks();
  calls.registry.mockResolvedValue(registry); calls.machines.mockResolvedValue([...machines, excluded]);
  calls.machine.mockImplementation((key) => {
    const machine = machines.find((row) => row.id === key);
    return machine === undefined ? Promise.reject(new Error('Unknown machine')) : Promise.resolve(machine);
  });
  calls.create.mockResolvedValue(detailAgent); calls.register.mockResolvedValue(detailAgent);
});

it('searches the full adapter registry and submits the raw selected kind only from Create', async () => {
  const user = userEvent.setup(), created = vi.fn();
  show(<AddAgentDialog initialHome="home" onClose={vi.fn()} onCreated={created} />);
  const input = await screen.findByRole('combobox', { name: i18n.t('agents.add.harness') });
  await user.click(input); expect(list(i18n.t('agents.add.harness')).getAllByRole('option')).toHaveLength(13);
  await user.type(input, 'adapter-13'); expect(list(i18n.t('agents.add.harness')).getAllByRole('option')).toHaveLength(1);
  fireEvent.keyDown(input, { key: 'Enter', keyCode: 229, isComposing: true });
  expect(calls.create).not.toHaveBeenCalled(); await user.keyboard('{Escape}'); expect(input).toHaveValue('');
  await user.click(input); await user.type(input, '__no_adapter__'); await user.keyboard('{Enter}');
  expect(calls.create).not.toHaveBeenCalled(); await user.clear(input); await user.type(input, 'adapter-13'); await user.keyboard('{Enter}');
  expect(input).toHaveValue('Registered adapter 13');
  await user.type(screen.getByLabelText(i18n.t('agents.add.name')), 'A deliberate profile');
  expect(calls.registry).toHaveBeenCalledOnce(); expect(calls.machines).not.toHaveBeenCalled(); expect(calls.create).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('agents.add.submit') }));
  await waitFor(() => expect(created).toHaveBeenCalledExactlyOnceWith(detailAgent));
  expect(calls.create).toHaveBeenCalledExactlyOnceWith('home', { name: 'A deliberate profile', harness_kind: 'adapter-13' }, expect.any(String));
  expect(calls.register).not.toHaveBeenCalled();
});

it('searches all eligible physical machines and their reported programs while retaining the offline reason', async () => {
  const user = userEvent.setup(), created = vi.fn();
  show(<AddAgentDialog initialHome="home" onClose={vi.fn()} onCreated={created} />);
  await user.click(field(i18n.t('agentCreation.mode'))); await user.click(screen.getByRole('option', { name: i18n.t('agentCreation.onMachine') }));
  const machineInput = await screen.findByRole('combobox', { name: i18n.t('agentCreation.machine') });
  await user.click(machineInput);
  const choices = list(i18n.t('agentCreation.machine'));
  expect(choices.getAllByRole('option')).toHaveLength(13); expect(choices.queryByRole('option', { name: excluded.name })).not.toBeInTheDocument();
  const offline = choices.getByRole('option', { name: 'Connected computer 1' });
  expect(offline).toHaveAttribute('aria-disabled', 'true'); expect(offline).toHaveAccessibleDescription(i18n.t('agentCreation.offline'));
  await user.click(offline); expect(calls.machine).not.toHaveBeenCalled();
  await user.type(machineInput, 'computer 13'); expect(choices.getAllByRole('option')).toHaveLength(1); await user.keyboard('{Enter}');
  expect(machineInput).toHaveValue('Connected computer 13');
  const adapter = await screen.findByRole('combobox', { name: i18n.t('agents.add.harness') });
  await user.click(adapter); expect(list(i18n.t('agents.add.harness')).getAllByRole('option')).toHaveLength(13);
  await user.type(adapter, 'adapter-13'); await user.keyboard('{Enter}'); expect(adapter).toHaveValue('Registered adapter 13');
  expect(calls.machines).toHaveBeenCalledOnce(); expect(calls.machine).toHaveBeenCalledExactlyOnceWith(id(90, 13));
  expect(calls.registry).toHaveBeenCalledOnce(); expect(calls.register).not.toHaveBeenCalled();
  await user.type(screen.getByLabelText(i18n.t('agents.add.name')), 'Machine profile');
  await user.click(screen.getByRole('button', { name: i18n.t('agents.add.submit') }));
  await waitFor(() => expect(created).toHaveBeenCalledExactlyOnceWith(detailAgent));
  expect(calls.register).toHaveBeenCalledExactlyOnceWith(id(90, 13), { home_id: 'home', name: 'Machine profile', harness_kind: 'adapter-13' }, expect.any(String));
  expect(calls.create).not.toHaveBeenCalled();
});
