/** Template forms preserve the recipe while editing named literal commands and all four registered program kinds. */
import { fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import { TemplateUnitFields } from '@/features/resources/templates/UnitFields';
import { TemplatePrograms } from '@/features/resources/UnitCommands';
import { i18n } from '@/i18n';
import { show } from './support';

function Form({ initial, kind = 'provider', onChange }: { initial: Record<string, unknown>; kind?: 'provider' | 'environment'; onChange: (next: Record<string, unknown>) => void }) {
  const [spec, setSpec] = useState(initial);
  return <TemplateUnitFields spec={spec} kind={kind} disabled={false} onChange={(next) => { setSpec(next); onChange(next); }} />;
}

it('converts legacy init without changing the image or splitting literal arguments', async () => {
  const user = userEvent.setup(); const change = vi.fn();
  show(<Form kind="environment" initial={{ image: 'example:1', needs: { memory_mb: 2048 }, init: { command: ['/opt/setup', 'one argument'], timeout_secs: 30 } }} onChange={change} />);
  const commands = screen.getByRole('region', { name: 'Commands after start' });
  expect(within(commands).getByLabelText('Command name')).toHaveValue('init');
  expect(within(commands).getByText(/The former init entry is superseded/)).toBeInTheDocument();
  await user.clear(within(commands).getByLabelText('Program'));
  await user.type(within(commands).getByLabelText('Program'), '/opt/new-setup');
  fireEvent.change(within(commands).getByLabelText('Arguments · one per line'), { target: { value: 'a b\n$HOME/bin\n--literal' } });
  expect(change).toHaveBeenLastCalledWith({ image: 'example:1', needs: { memory_mb: 2048 }, after_up: [{ name: 'init', command: ['/opt/new-setup', 'a b', '$HOME/bin', '--literal'], timeout_secs: 30 }] });
  expect(screen.queryByRole('region', { name: 'Registered programs' })).not.toBeInTheDocument();
});

it('edits provider programs with their own variables and restart choice, preserving other kinds', () => {
  const change = vi.fn();
  const kinds = ['provider', 'translator', 'adapter', 'executor'] as const;
  const programs = kinds.map((kind) => ({ kind, name: `team-${kind}`, command: [`/opt/${kind}`, 'a b'], env: { MODE: 'literal' }, restart: 'on-failure' }));
  show(<Form initial={{ image: 'example:1', plugins: programs }} onChange={change} />);
  const region = screen.getByRole('region', { name: 'Registered programs' });
  for (const [index, kind] of kinds.entries()) {
    const program = within(region).getByRole('group', { name: `Registered program ${index + 1}` });
    expect(within(program).getByRole('combobox', { name: 'Program kind' })).toHaveTextContent(i18n.t(`templates.choices.plugins.*.kind.${kind}.name`));
  }
  const adapter = within(region).getByRole('group', { name: 'Registered program 3' });
  fireEvent.change(within(adapter).getByLabelText('Arguments · one per line'), { target: { value: 'space in this argument\n--mode=literal' } });
  expect(change).toHaveBeenLastCalledWith({ image: 'example:1', plugins: [programs[0], programs[1], { ...programs[2], command: ['/opt/adapter', 'space in this argument', '--mode=literal'] }, programs[3]] });
  expect(within(adapter).getByDisplayValue('MODE')).toBeInTheDocument();
  expect(within(adapter).getByDisplayValue('literal')).toBeInTheDocument();
  expect(within(adapter).getByRole('combobox', { name: 'Restart policy' })).toHaveTextContent(i18n.t('templates.choices.plugins.*.restart.on-failure.name'));
});

it('a reader sees template programs and command fields without controls that can save or add', () => {
  const spec = { image: 'example:1', after_up: [{ name: 'setup', command: ['/opt/setup'], timeout_secs: 45 }], plugins: [{ kind: 'executor', name: 'team-exec', command: ['/opt/exec', 'a b'] }] };
  const change = vi.fn();
  show(<TemplateUnitFields spec={spec} kind="provider" disabled onChange={change} />);
  expect(screen.getByLabelText('Command name')).toBeDisabled();
  expect(screen.getByLabelText('Registered name')).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Add command' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Add program' })).toBeDisabled();
  expect(change).not.toHaveBeenCalled();
});

it('the runtime lists each declared program with its kind and literal command, read-only', () => {
  const plugins = ['provider', 'translator', 'adapter', 'executor'].map((kind) => ({ kind, name: `team-${kind}`, command: [`/opt/${kind}`, 'one argument'] }));
  show(<TemplatePrograms spec={{ plugins }} />);
  const list = screen.getByRole('region', { name: 'Registered programs' });
  expect(within(list).getAllByRole('listitem')).toHaveLength(4);
  for (const program of plugins) expect(within(list).getByText(JSON.stringify(program.command))).toBeInTheDocument();
  expect(within(list).getByText(/Read-only · declared by this runtime's template/)).toBeInTheDocument();
  expect(within(list).queryByRole('textbox')).not.toBeInTheDocument();
  expect(within(list).queryByRole('button')).not.toBeInTheDocument();
});
