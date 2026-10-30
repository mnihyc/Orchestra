/** The pieces of the two screens that need nothing from the server: the groups of a provider's models and the way a limit is opened from them, and the choices of speed, access and context of a thread's model control with where each comes from. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';

import { ContextChoice } from '@/features/composer/ContextChoice';
import { type ContextState, type SpeedOrAccess, statesOf } from '@/features/composer/modelState';
import { AccessChoice, SpeedChoice } from '@/features/composer/SpeedAndAccess';
import { GroupsField, GroupStrip } from '@/features/resources/providers/Groups';
import type { ModelParameters, ProviderModel, ProviderRef, ThreadParameters } from '@/generated/api-types';

import { show } from './support';
import { MODEL_ESTIMATE } from '../doubles/fixtures';

const PROVIDER = { address: 'acme.ada', harness_kind: 'codex', id: 'p1', kind: 'subscription', name: 'Team Pro', placement: 'managed', status: 'ready' } as ProviderRef;

const model = (name: string, groups: string[]): ProviderModel => ({ ...MODEL_ESTIMATE, id: name, model_name: name, display_name: '', enabled: true, source: 'manual', groups, parameters: {} });

describe('the groups of a provider\'s models', () => {
  it('are listed above the list with how many models each has, the largest first, and are left out for a reader who may not manage them when there are none', () => {
    const models = [model('a', ['small']), model('b', ['frontier', 'small']), model('c', ['small'])];
    const { unmount } = show(<GroupStrip provider={PROVIDER} models={models} manage />);
    const strip = screen.getByRole('region', { name: 'Groups' });
    const rows = within(strip).getAllByRole('listitem');
    expect(rows.map((row) => row.textContent)).toEqual([expect.stringContaining('small3 models'), expect.stringContaining('frontier1 model')]);
    unmount();

    show(<GroupStrip provider={PROVIDER} models={[model('a', [])]} manage={false} />);
    expect(screen.queryByRole('region', { name: 'Groups' })).not.toBeInTheDocument();
  });

});

function Groups({ start, offered = [], disabled = false }: { start: string[]; offered?: string[]; disabled?: boolean }) {
  const [groups, setGroups] = useState(start);
  return <GroupsField groups={groups} offered={offered} disabled={disabled} onChange={setGroups} />;
}

describe('the groups of one model', () => {
  it('are put in by typing, one at a time, and a name that is no group name is refused where it was typed', async () => {
    show(<Groups start={['frontier']} />);
    const field = screen.getByLabelText('Put it in a group');
    await userEvent.type(field, 'Big Group{Enter}');
    expect(screen.getByText(/A group is named by 1 to 40/)).toBeInTheDocument();
    expect(screen.queryByText('Big Group')).not.toBeInTheDocument();

    await userEvent.clear(field);
    await userEvent.type(field, 'vision{Enter}');
    const chips = screen.getByRole('list', { name: 'Groups of this model' });
    expect(within(chips).getByText('vision')).toBeInTheDocument();
    expect(within(chips).getByText('frontier')).toBeInTheDocument();
    expect(field).toHaveValue('');
  });

  it("are taken from the provider's others, and taken out", async () => {
    show(<Groups start={['frontier']} offered={['frontier', 'small', 'vision']} />);
    const others = screen.getByRole('group', { name: 'Other groups of this provider' });
    // What the model is in is not offered again.
    expect(within(others).queryByRole('button', { name: 'frontier' })).not.toBeInTheDocument();
    await userEvent.click(within(others).getByRole('button', { name: 'small' }));
    const chips = screen.getByRole('list', { name: 'Groups of this model' });
    expect(within(chips).getByText('small')).toBeInTheDocument();
    const remove = screen.getByRole('button', { name: 'Take it out of the group frontier' });
    expect(remove).toHaveAttribute('title', 'Take it out of the group frontier');
    expect(remove.querySelector('[data-action-icon="delete"]')).toBeInTheDocument();
    await userEvent.click(remove);
    expect(within(chips).queryByText('frontier')).not.toBeInTheDocument();
  });

  it('stop at eight', () => {
    show(<Groups start={['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h']} offered={['i']} />);
    expect(screen.getByLabelText('Put it in a group')).toBeDisabled();
    expect(screen.getByText('A model is in at most 8 groups.')).toBeInTheDocument();
  });

  it('are only read for somebody who may not change them', () => {
    show(<Groups start={['frontier']} disabled />);
    expect(screen.queryByLabelText('Put it in a group')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Take it out/ })).not.toBeInTheDocument();
    expect(screen.getByText('frontier')).toBeInTheDocument();
  });
});

// ---------------------------------------------------------------- the choices of a thread

const SPEED = { key: 'service_tier', title: 'Speed', description: 'A faster mode.', type: 'enum', options: { from_model: 'service_tiers' }, default: { from_model: 'default_service_tier' }, apply: 'next_turn' };
const ACCESS = { key: 'cyber_access_program', title: 'Access program', description: 'A program.', type: 'enum', options: { from_model: 'access_programs' }, default: { from_model: 'default_access_program' }, apply: 'next_turn' };
const CONTEXT = { key: 'context', title: 'Context', description: 'Shortened before it reaches that size.', type: 'integer', range: { min: 32_000, max: { from_model: 'context_max' } }, default: { from_model: 'default_context' }, apply: 'new_session' };

const SOL: ModelParameters = {
  context_window: 272_000,
  context_max: 1_000_000,
  service_tiers: [{ id: 'fast', label: 'Fast' }],
  access_programs: ['Daybreak Blue'],
  rate: { input_per_mtok: '2.5', cached_input_per_mtok: '0.25', output_per_mtok: '15', tiers: { fast: '2' }, above: [{ context_tokens: 272_000, input_per_mtok: '5', output_per_mtok: '22.5' }] },
};

const statesFor = (entry: ModelParameters, own: Record<string, unknown> = {}) =>
  statesOf(
    {
      effort: { value: null, source: { scope: 'default', node: null }, writable: true, supported: true },
      model: { value: 'sol', source: { scope: 'default', node: null }, writable: true, options: [{ allowed: true, display_name: 'Sol', model_name: 'sol', parameters: entry, reason: null }] },
      parameters: [SPEED, ACCESS, CONTEXT].map((def) => ({ key: def.key, def, value: own[def.key] ?? null, source: { scope: own[def.key] === undefined ? 'default' : 'thread', node: null }, locked_by: null, writable: true, apply: def.apply })),
    } as unknown as ThreadParameters,
    own,
  );

const speed = (entry: ModelParameters, own: Record<string, unknown> = {}) => statesFor(entry, own).find((state): state is SpeedOrAccess => state.kind === 'speed')!;
const access = (entry: ModelParameters, own: Record<string, unknown> = {}) => statesFor(entry, own).find((state): state is SpeedOrAccess => state.kind === 'access')!;
const context = (entry: ModelParameters, own: Record<string, unknown> = {}) => statesFor(entry, own).find((state): state is ContextState => state.kind === 'context')!;

describe('the speed of a thread', () => {
  it('reads Standard where the model\'s managers switched Fast on and nobody chose it, with Fast as a choice and what it costs', async () => {
    const choose = vi.fn();
    const user = userEvent.setup();
    show(<SpeedChoice state={speed(SOL)} writable chooser={null} rate={SOL.rate ?? null} onChoose={choose} />);
    expect(screen.getByRole('radio', { name: 'Standard' })).toBeChecked();
    expect(screen.getByText(/as this model starts/)).toBeInTheDocument();
    expect(screen.queryByText('Fast: quicker answers, counted at twice the rate here')).not.toBeInTheDocument();
    expect(screen.getByText('Takes effect from the next turn.')).toBeInTheDocument();
    const fast = screen.getByRole('radio', { name: 'Fast' });
    await user.hover(fast);
    expect(await screen.findByRole('tooltip')).toHaveTextContent('Fast: quicker answers, counted at twice the rate here');
    await user.click(fast);
    expect(choose).toHaveBeenCalledWith('fast');
  });

  it('says who chose it, and that it is the thread\'s own', () => {
    show(<SpeedChoice state={speed(SOL, { service_tier: 'fast' })} writable chooser={{ name: 'Dana' }} rate={null} onChoose={() => undefined} />);
    expect(screen.getByRole('radio', { name: 'Fast' })).toBeChecked();
    expect(screen.getByText(/chosen by Dana/)).toBeInTheDocument();
  });

  it('offers no way back to Standard where the model\'s managers made Fast its start', () => {
    show(<SpeedChoice state={speed({ ...SOL, default_service_tier: 'fast' })} writable chooser={null} rate={null} onChoose={() => undefined} />);
    expect(screen.getByRole('radio', { name: 'Fast' })).toBeChecked();
    expect(screen.getByRole('radio', { name: 'Standard' })).toBeDisabled();
    expect(screen.getByText(/start with Fast, as its managers chose/)).toBeInTheDocument();
  });

  it('is read and not changed by somebody who may not set the thread up', () => {
    show(<SpeedChoice state={speed(SOL)} writable={false} chooser={null} rate={null} onChoose={() => undefined} />);
    expect(screen.getByRole('radio', { name: 'Standard' })).toBeDisabled();
    expect(screen.getByRole('radio', { name: 'Fast' })).toBeDisabled();
  });

  it('flags a choice that is no longer open', () => {
    show(<SpeedChoice state={speed(SOL, { service_tier: 'priority' })} writable chooser={null} rate={null} onChoose={() => undefined} />);
    expect(screen.getByText(/no longer open, so this model's start is used/)).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: 'Standard' })).toBeChecked();
  });
});

describe('the access program of a thread', () => {
  it('reads None until somebody chooses a program, and names what a choice comes to', () => {
    const { unmount } = show(<AccessChoice state={access(SOL)} writable chooser={null} onChoose={() => undefined} />);
    expect(screen.getByRole('combobox', { name: 'Access' })).toHaveTextContent('None');
    expect(screen.getByText('None: no access program is asked for.')).toBeInTheDocument();
    unmount();
    show(<AccessChoice state={access(SOL, { cyber_access_program: 'Daybreak Blue' })} writable chooser={null} onChoose={() => undefined} />);
    expect(screen.getByRole('combobox', { name: 'Access' })).toHaveTextContent('Daybreak Blue');
    expect(screen.getByText('Daybreak Blue: this thread\'s turns ask for that access program.')).toBeInTheDocument();
  });
});

describe('the context of a thread', () => {
  it('opens on all that is open, with the sizes between the model\'s own window and the largest, and says where the conversation is shortened in the adapter\'s words', () => {
    show(<ContextChoice state={context(SOL)} writable chooser={null} onChoose={() => undefined} />);
    expect(screen.getByRole('radio', { name: /^1M/ })).toBeChecked();
    expect(screen.getByRole('radio', { name: '272K' })).toBeInTheDocument();
    expect(screen.getByText(/as this model starts/)).toBeInTheDocument();
    expect(screen.getByText('Shortened before it reaches that size.')).toBeInTheDocument();
    expect(screen.getByText('Takes effect in a new session.')).toBeInTheDocument();
  });

  it('names higher-rate sizes compactly and explains their cost in the option tooltip', async () => {
    show(<ContextChoice state={context(SOL)} writable chooser={null} onChoose={() => undefined} />);
    const selected = screen.getByRole('radio', { name: '1M · higher rate' });
    expect(selected).toBeChecked();
    expect(screen.queryByText('Prompts above 272K are counted at a higher rate.')).not.toBeInTheDocument();
    await userEvent.hover(selected);
    expect(await screen.findByRole('tooltip')).toHaveTextContent('Prompts above 272K are counted at a higher rate.');
    expect(selected).toHaveAccessibleDescription('Prompts above 272K are counted at a higher rate.');
    expect(document.querySelector('[data-choice-selected-description]')).toBeNull();
    // The model's own window is no size above the step.
    expect(screen.getByRole('radio', { name: '272K' })).toBeInTheDocument();
  });

  it('says nothing of a step where the model has no rate of its own', () => {
    show(<ContextChoice state={context({ ...SOL, rate: null })} writable chooser={null} onChoose={() => undefined} />);
    expect(screen.queryByText(/higher rate/)).not.toBeInTheDocument();
  });

  it('takes another number in its own field, within what is open, and refuses the rest where it was typed', async () => {
    const choose = vi.fn();
    show(<ContextChoice state={context(SOL)} writable chooser={null} onChoose={choose} />);
    const field = screen.getByLabelText('Another size, in tokens');
    await userEvent.type(field, '1000{Enter}');
    expect(screen.getByText('At least 32K.')).toBeInTheDocument();
    await userEvent.clear(field);
    await userEvent.type(field, '2000000{Enter}');
    expect(screen.getByText('At most 1M, the largest open for this model.')).toBeInTheDocument();
    await userEvent.clear(field);
    await userEvent.type(field, '1.5{Enter}');
    expect(screen.getByText('Write a whole number of tokens.')).toBeInTheDocument();
    expect(choose).not.toHaveBeenCalled();
    await userEvent.clear(field);
    await userEvent.type(field, '750000{Enter}');
    expect(choose).toHaveBeenCalledWith(750_000);
  });

  it('shows a size that was typed in as a choice of its own, and says the thread chose it', () => {
    show(<ContextChoice state={context(SOL, { context: 750_000 })} writable chooser={{ name: 'Dana' }} onChoose={() => undefined} />);
    expect(screen.getByRole('radio', { name: /^750K/ })).toBeChecked();
    expect(screen.getByText(/chosen by Dana/)).toBeInTheDocument();
  });

  it('is only read where the reader may not change it, and says which place decides it where one does', () => {
    const state = { ...context(SOL), lockedBy: { scope: 'project' as const, node: { id: 'n', kind: 'project' as const, name: 'Web', redacted: false } }, writable: false };
    show(<ContextChoice state={state} writable={false} chooser={null} onChoose={() => undefined} />);
    expect(screen.getByRole('radio', { name: /^1M/ })).toBeDisabled();
    expect(screen.queryByLabelText('Another size, in tokens')).not.toBeInTheDocument();
    expect(screen.getByText('Web decides this for its threads, so it cannot be changed here.')).toBeInTheDocument();
  });
});
