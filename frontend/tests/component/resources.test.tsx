/** The controls that resources added to the design system: a measure that says how full something is, a choice whose options each explain themselves, and a form drawn from the fields a driver declares. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';

import type { Field } from '@/generated/api-types';
import { FieldForm, type FormValue, type FormValues, initialValues } from '@/ui/FieldForm';
import { Meter } from '@/ui/Meter';
import { RadioGroup } from '@/ui/RadioGroup';

import { show } from './support';

describe('a measure', () => {
  it('says how much is used, to the eye and to a screen reader, and holds a figure that is outside 0 to 100 to it', () => {
    show(<Meter value={140} label="Memory declared" detail="9 GiB of 8 GiB" />);

    const meter = screen.getByRole('meter', { name: 'Memory declared' });
    expect(meter).toHaveAttribute('aria-valuenow', '100');
    expect(meter).toHaveAttribute('aria-valuetext', '100%');
    expect(screen.getByText('9 GiB of 8 GiB')).toBeInTheDocument();
    expect(screen.getByText('100%')).toBeInTheDocument();
  });

  it('rounds the figure it writes, and is nothing at all for a figure that is not a number', () => {
    const { rerender } = show(<Meter value={32.5} label="Disk" />);
    expect(screen.getByRole('meter', { name: 'Disk' })).toHaveAttribute('aria-valuenow', '33');
    rerender(<Meter value={Number.NaN} label="Disk" />);
    expect(screen.getByRole('meter', { name: 'Disk' })).toHaveAttribute('aria-valuenow', '0');
  });

  it('is only a bar and its figure where it stands in a cell of a table, and its label is then only spoken', () => {
    show(<Meter look="compact" value={6} label="Memory declared" />);
    expect(screen.getByRole('meter', { name: 'Memory declared' })).toBeInTheDocument();
    expect(screen.queryByText('Memory declared')).not.toBeInTheDocument();
    expect(screen.getByText('6%')).toBeInTheDocument();
  });
});

type Mode = 'warn' | 'block' | 'system';

function Choice({ onPick, startAt = null }: { onPick?: (value: Mode) => void; startAt?: Mode | null }) {
  const [value, setValue] = useState<Mode | null>(startAt);
  return (
    <RadioGroup<Mode>
      label="When it is full"
      variant="list"
      value={value}
      onValueChange={(next) => {
        setValue(next);
        onPick?.(next);
      }}
      options={[
        { value: 'warn', label: 'Warns', description: 'It can still be chosen.' },
        { value: 'block', label: 'Blocks', description: 'It is not chosen.', note: 'Its own answer.' },
        { value: 'system', label: 'As the system decides', disabled: true, note: 'Nothing to follow yet.' },
      ]}
    />
  );
}

describe('a choice whose options explain themselves', () => {
  it('keeps ordinary options compact and marks the chosen one without description blocks', () => {
    show(<Choice startAt="block" />);

    const group = screen.getByRole('radiogroup', { name: 'When it is full' });
    expect(within(group).getByText('It can still be chosen.')).toBeVisible();
    expect(within(group).getByText('Its own answer.')).toBeVisible();
    expect(within(group).getByRole('radio', { name: 'Blocks' })).toHaveAccessibleName('Blocks');
    expect(within(group).getByRole('radio', { name: /Blocks/ })).toBeChecked();
    expect(within(group).getByRole('radio', { name: /Warns/ })).not.toBeChecked();
  });

  it('is chosen by pointer, and says which option was chosen', async () => {
    const user = userEvent.setup();
    const onPick = vi.fn();
    show(<Choice onPick={onPick} />);

    await user.click(screen.getByRole('radio', { name: /Warns/ }));
    expect(onPick).toHaveBeenLastCalledWith('warn');
    await user.click(screen.getByRole('radio', { name: /Blocks/ }));
    expect(onPick).toHaveBeenLastCalledWith('block');
    expect(screen.getByRole('radio', { name: /Blocks/ })).toBeChecked();
    expect(screen.getByRole('radio', { name: /Warns/ })).not.toBeChecked();
  });

  it('cannot be chosen where it is disabled and keeps its reason accessible without a description block', async () => {
    const user = userEvent.setup();
    const onPick = vi.fn();
    show(<Choice onPick={onPick} />);

    const off = screen.getByRole('radio', { name: 'As the system decides' });
    expect(off).toHaveAccessibleDescription('Nothing to follow yet.');
    expect(off).toBeDisabled();
    expect(off).toHaveAttribute('title', 'Nothing to follow yet.');
    expect(screen.getByText('Nothing to follow yet.')).toBeVisible();
    await user.click(off);
    expect(onPick).not.toHaveBeenCalled();
  });
});

const FIELDS: Field[] = [
  { name: 'dockerfile', label: 'The Dockerfile', type: 'secret_text', required: true, help: 'Pasted or chosen as a file.', max_bytes: 4096, choices: null, default: null },
  { name: 'token', label: 'Token', type: 'secret', required: true, help: null, max_bytes: null, choices: null, default: null },
  { name: 'tag', label: 'Tag', type: 'text', required: false, help: null, max_bytes: null, choices: null, default: 'latest' },
  { name: 'pull', label: 'Pull first', type: 'boolean', required: false, help: 'Fetches the base image again.', max_bytes: null, choices: null, default: false },
  { name: 'runtime', label: 'Runtime', type: 'choice', required: false, help: null, max_bytes: null, choices: [{ value: 'runc', label: 'runc' }, { value: 'runsc', label: 'gVisor' }], default: null },
  { name: 'args', label: 'Build arguments', type: 'map', required: false, help: null, max_bytes: null, choices: null, default: null },
  { name: 'headers', label: 'Secret headers', type: 'secret_map', required: false, help: null, max_bytes: null, choices: null, default: null },
] as unknown as Field[];

function Declared({ onChange, errors }: { onChange?: (name: string, value: FormValue) => void; errors?: Record<string, string> }) {
  const [values, setValues] = useState<FormValues>(() => initialValues(FIELDS));
  return (
    <FieldForm
      fields={FIELDS}
      values={values}
      errors={errors ?? {}}
      onChange={(name, value) => {
        setValues((before) => ({ ...before, [name]: value }));
        onChange?.(name, value);
      }}
    />
  );
}

describe('a form drawn from declared fields', () => {
  it('starts each field at what it declares and draws the help of the fields that have any', () => {
    show(<Declared />);

    expect(screen.getByLabelText(/^Tag/)).toHaveValue('latest');
    expect(screen.getByText('Pasted or chosen as a file.')).toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: 'Pull first' })).not.toBeChecked();
    // A secret is a hidden input, and a field that declares no help has no line under it.
    expect(screen.getByLabelText('Token')).toHaveAttribute('type', 'password');
  });

  it('hands back what is typed, field by field', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    show(<Declared onChange={onChange} />);

    await user.clear(screen.getByLabelText(/^Tag/));
    await user.type(screen.getByLabelText(/^Tag/), 'v2');
    expect(onChange).toHaveBeenLastCalledWith('tag', 'v2');
    await user.click(screen.getByRole('checkbox', { name: 'Pull first' }));
    expect(onChange).toHaveBeenLastCalledWith('pull', true);
  });

  it('says what is wrong with a field beside it, and ties that to the field', () => {
    show(<Declared errors={{ token: 'Give the token.' }} />);

    const field = screen.getByLabelText('Token');
    expect(screen.getByText('Give the token.')).toBeInTheDocument();
    expect(field).toHaveAccessibleDescription(expect.stringContaining('Give the token.'));
  });

  it('keeps the rows of a map as a name and a value, adds one at a time, and hides the values of a secret map', async () => {
    const user = userEvent.setup();
    show(<Declared />);

    const args = screen.getByRole('group', { name: /Build arguments/ });
    await user.click(within(args).getByRole('button', { name: 'Add to Build arguments' }));
    // A row with no name yet stops another from being added: names are never merged while they are typed.
    expect(within(args).getByRole('button', { name: 'Add to Build arguments' })).toBeDisabled();
    await user.type(within(args).getByPlaceholderText('Name'), 'NODE_ENV');
    await user.type(within(args).getByPlaceholderText('Value'), 'production');
    expect(within(args).getByDisplayValue('NODE_ENV')).toBeInTheDocument();
    expect(within(args).getByDisplayValue('production')).toBeInTheDocument();

    const headers = screen.getByRole('group', { name: /Secret headers/ });
    await user.click(within(headers).getByRole('button', { name: 'Add to Secret headers' }));
    expect(within(headers).getByPlaceholderText('Value')).toHaveAttribute('type', 'password');

    await user.click(within(args).getByRole('button', { name: /Remove NODE_ENV/ }));
    expect(within(args).queryByDisplayValue('NODE_ENV')).not.toBeInTheDocument();
    // Typing letter by letter into a form that draws again at each one is slow on a busy machine.
  }, 30_000);
});
