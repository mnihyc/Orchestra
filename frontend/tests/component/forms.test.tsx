/** A form: what is wrong is said when a field is left and when the form is sent, and the keyboard is put on the first field to mend. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import type { Problem } from '@/generated/api-types';
import { useForm } from '@/lib/forms';
import { Input, PasswordInput } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { useState } from 'react';

import { show } from './support';

function StatusPicker() {
  const [status, setStatus] = useState('open');
  return <Select label="Status" hideLabel size="sm" value={status} valueLabel={`Status: ${status === 'open' ? 'Open' : 'Done'}`}
    onValueChange={setStatus} options={[{ value: 'open', label: 'Open' }, { value: 'done', label: 'Done' }]} />;
}

function Example({ onSend, refusal }: { onSend: (values: { name: string; secret: string }) => void; refusal?: unknown }) {
  const form = useForm(
    { name: '', secret: '' },
    {
      name: (value) => (value.trim() === '' ? 'Enter a name.' : null),
      secret: (value) => (value.length < 4 ? 'Use at least 4 characters.' : null),
    },
  );
  return (
    <form
      noValidate
      onSubmit={form.submit((values) => {
        onSend(values);
        if (refusal !== undefined) form.refuse(refusal);
      })}
    >
      <Input label="Name" {...form.field('name')} />
      <PasswordInput label="Secret" hint="Four characters or more." {...form.field('secret')} />
      <button type="submit">Send</button>
      <button type="button" onClick={() => form.focus('name')}>
        Go to the name
      </button>
    </form>
  );
}

describe('a form', () => {
  it('QS-019 names a hidden-label select by its field and shows its status with the value', async () => {
    const user = userEvent.setup();
    show(<StatusPicker />);
    const picker = screen.getByRole('combobox', { name: /^Status$/ });
    expect(picker).toHaveTextContent('Status: Open');
    await user.click(picker);
    await user.click(screen.getByRole('option', { name: /^Done$/ }));
    expect(picker).toHaveTextContent('Status: Done');
    expect(picker).toHaveAccessibleName('Status');
  });
  it('says nothing about a field while its reader is still in it for the first time', async () => {
    const user = userEvent.setup();
    show(<Example onSend={vi.fn()} />);

    await user.type(screen.getByLabelText('Secret'), 'ab');

    expect(screen.queryByText('Use at least 4 characters.')).not.toBeInTheDocument();
    expect(screen.getByText('Four characters or more.')).toBeInTheDocument();
  });

  it('says what is wrong with a field when its reader leaves it, in place of the hint, and ties it to the field', async () => {
    const user = userEvent.setup();
    show(<Example onSend={vi.fn()} />);

    await user.type(screen.getByLabelText('Secret'), 'ab');
    await user.tab();

    const field = screen.getByLabelText('Secret');
    expect(field).toHaveAccessibleDescription('Use at least 4 characters.');
    expect(field).toBeInvalid();
    expect(screen.queryByText('Four characters or more.')).not.toBeInTheDocument();
  });

  it('sends nothing while something is wrong, and puts the keyboard on the first field to mend', async () => {
    const user = userEvent.setup();
    const onSend = vi.fn();
    show(<Example onSend={onSend} />);

    await user.type(screen.getByLabelText('Secret'), 'long enough');
    await user.click(screen.getByRole('button', { name: 'Send' }));

    expect(onSend).not.toHaveBeenCalled();
    expect(screen.getByText('Enter a name.')).toBeInTheDocument();
    expect(screen.getByLabelText('Name')).toHaveFocus();

    await user.type(screen.getByLabelText('Name'), 'Ada');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    expect(onSend).toHaveBeenCalledWith({ name: 'Ada', secret: 'long enough' });
  });

  it('shows what the server found wrong at the field it names, until that field is changed', async () => {
    const user = userEvent.setup();
    const problem = {
      type: 'orch:error:request.invalid',
      code: 'request.invalid',
      title: 'Not valid',
      status: 400,
      detail: 'not valid',
      request_id: 'r1',
      errors: [{ path: 'name', message: 'is taken by an org' }],
    } satisfies Problem;
    show(<Example onSend={vi.fn()} refusal={new ApiError(problem, null)} />);

    await user.type(screen.getByLabelText('Name'), 'acme');
    await user.type(screen.getByLabelText('Secret'), 'long enough');
    await user.click(screen.getByRole('button', { name: 'Send' }));

    expect(screen.getByLabelText('Name')).toHaveAccessibleDescription('is taken by an org');
    expect(screen.getByLabelText('Name')).toHaveFocus();

    await user.type(screen.getByLabelText('Name'), '2');
    expect(screen.queryByText('is taken by an org')).not.toBeInTheDocument();
  });

  it('puts the keyboard on a field when asked to', async () => {
    const user = userEvent.setup();
    show(<Example onSend={vi.fn()} />);

    await user.click(screen.getByRole('button', { name: 'Go to the name' }));

    expect(screen.getByLabelText('Name')).toHaveFocus();
  });

  it('shows a password when its reader asks to see it', async () => {
    const user = userEvent.setup();
    show(<Example onSend={vi.fn()} />);
    const secret = screen.getByLabelText('Secret');
    await user.type(secret, 'hunter2');

    expect(secret).toHaveAttribute('type', 'password');
    await user.click(screen.getByRole('button', { name: 'Show Secret' }));
    expect(secret).toHaveAttribute('type', 'text');
    expect(screen.getByRole('button', { name: 'Hide Secret' })).toHaveAttribute('aria-pressed', 'true');
  });
});
