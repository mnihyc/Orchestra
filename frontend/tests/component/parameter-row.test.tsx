/** Native parameters keep compact labels in the page and put dropdown explanations inside the open list. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { ParameterRow } from '@/features/inspector/Parameters';
import type { RunsWith } from '@/features/composer/runsWith';
import type { ParameterView } from '@/generated/api-types';
import { me, userRef } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@/features/help/SettingHint', () => ({ SettingHelpLink: () => null }));
const access = { thread: { id: 'thread-1', owner: userRef(me(1, 'ada', 'Ada')) } };
const explanation = 'A long native explanation belongs in the picker rather than beside the switch.';
function runs() {
  return { ready: true, write: vi.fn<RunsWith['write']>().mockResolvedValue(undefined), reset: vi.fn<RunsWith['reset']>().mockResolvedValue(undefined), refuse: vi.fn<RunsWith['refuse']>().mockResolvedValue(undefined) };
}
function parameter(more: Partial<ParameterView> = {}): ParameterView {
  return { key: 'agent.params.memory', def: { title: 'Use memory', description: explanation, type: 'bool' }, value: false,
    apply: 'next_turn', source: { scope: 'default', node: null }, locked_by: null, writable: true, ...more };
}

it('shows the boolean name once, without a description block, and keeps its guarded write', async () => {
  const writer = runs();
  show(<ul><ParameterRow parameter={parameter()} access={access} writable runs={writer} /></ul>);
  expect(screen.getAllByText('Use memory')).toHaveLength(1);
  expect(screen.queryByText(explanation)).not.toBeInTheDocument();
  const control = screen.getByRole('switch', { name: 'Use memory' });
  expect(control).not.toBeChecked();
  await userEvent.click(control);
  await waitFor(() => expect(writer.write).toHaveBeenCalledExactlyOnceWith('agent.params.memory', true));
});

it('retains descriptions only in the opened dropdown and sends the named option', async () => {
  const writer = runs();
  show(<ul><ParameterRow parameter={parameter({ key: 'agent.params.style', value: 'careful', def: { title: 'Work style', description: explanation, type: 'string', options: [{ value: 'careful', label: 'Careful' }, { value: 'quick', label: 'Quick' }] } })} access={access} writable runs={writer} /></ul>);
  expect(screen.queryByText(explanation)).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('combobox', { name: 'Work style' }));
  expect(screen.getAllByText(explanation).length).toBeGreaterThan(0);
  await userEvent.click(screen.getByRole('option', { name: 'Quick' }));
  await waitFor(() => expect(writer.write).toHaveBeenCalledExactlyOnceWith('agent.params.style', 'quick'));
});

it('keeps an unsupported reader’s switch read-only with the existing reason', async () => {
  const writer = runs();
  show(<ul><ParameterRow parameter={parameter()} access={access} writable={false} runs={writer} /></ul>);
  const control = screen.getByRole('switch', { name: 'Use memory' });
  expect(control).toBeDisabled();
  await userEvent.click(control);
  expect(writer.write).not.toHaveBeenCalled();
  expect(screen.getByText(/Ada/)).toBeVisible();
});

const manyOptions = Array.from({ length: 12 }, (_, index) => ({ value: `native-${index + 1}`, label: `Style ${index + 1}` }));

it('locates a deep native option without writes on typing, then reopens after a stale-write refusal', async () => {
  const user = userEvent.setup(), writer = runs();
  const refusal = new ApiError({ type: 'orch:error:conflict.stale_revision', title: 'Changed', status: 409,
    code: 'conflict.stale_revision', detail: 'The parameter changed elsewhere.', request_id: 'parameter-locator' }, null);
  writer.write.mockRejectedValueOnce(refusal);
  show(<ul><ParameterRow parameter={parameter({ key: 'agent.params.style', value: null,
    def: { title: 'Work style', description: explanation, type: 'string', default: 'native-11', options: manyOptions } })}
    access={access} writable runs={writer} /></ul>);
  const input = screen.getByRole('combobox', { name: 'Work style' });
  expect(input).toHaveValue('Style 11');
  await user.click(input);
  expect(screen.getAllByRole('option')).toHaveLength(12);
  await user.type(input, 'native-12');
  expect(writer.write).not.toHaveBeenCalled();
  expect(writer.refuse).not.toHaveBeenCalled();
  expect(writer.reset).not.toHaveBeenCalled();
  expect(screen.getByRole('option', { name: 'Style 12' })).toHaveAccessibleDescription(explanation);
  await user.keyboard('{Enter}');
  await waitFor(() => expect(writer.refuse).toHaveBeenCalledExactlyOnceWith(refusal, 'agent.params.style'));
  await waitFor(() => expect(input).toHaveAttribute('aria-expanded', 'true'));
  expect(input).toHaveFocus();
  expect(screen.getAllByRole('option')).toHaveLength(12);
  expect(writer.write).toHaveBeenCalledExactlyOnceWith('agent.params.style', 'native-12');
  await user.type(input, 'native-12');
  expect(writer.write).toHaveBeenCalledTimes(1);
  await user.keyboard('{Enter}');
  await waitFor(() => expect(writer.write).toHaveBeenCalledTimes(2));
  expect(writer.write).toHaveBeenLastCalledWith('agent.params.style', 'native-12');
});

it('shows an unknown current parameter under its native lock without enabling a write', async () => {
  const writer = runs();
  show(<ul><ParameterRow parameter={parameter({ key: 'agent.params.style', value: 'future-style',
    locked_by: { scope: 'org', node: { id: 'org', kind: 'org', name: 'Research', redacted: false } },
    def: { title: 'Work style', description: explanation, type: 'string', options: manyOptions } })}
    access={access} writable runs={writer} /></ul>);
  const input = screen.getByRole('combobox', { name: 'Work style' });
  expect(input).toHaveValue('future-style');
  expect(input).toBeDisabled();
  expect(screen.getByText(/Research/)).toBeVisible();
  await userEvent.click(input);
  expect(screen.queryByRole('option')).not.toBeInTheDocument();
  expect(writer.write).not.toHaveBeenCalled();
});
