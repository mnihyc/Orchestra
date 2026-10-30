/** A returned program can add and select an option in one render without a native mirror clearing it. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { StrictMode, useState } from 'react';
import { expect, it, vi } from 'vitest';

import { Button } from '@/ui/Button';
import { Select, type SelectOption } from '@/ui/Select';
import { show } from './support';

function Profile({ changed, submitted }: { changed: (value: string) => void; submitted: (value: string) => void }) {
  const [value, setValue] = useState('');
  const [options, setOptions] = useState<SelectOption<string>[]>([]);
  return <form aria-label="Agent profile" onSubmit={(event) => { event.preventDefault(); submitted(value); }}>
    <Select label="Agent program" placeholder="Choose a program" value={value} options={options}
      onValueChange={(next) => { changed(next); setValue(next); }} />
    <Button onClick={() => {
      setOptions([{ value: 'codex', name: 'Codex' }, { value: 'research-agent', name: 'Research agent' }]);
      setValue('research-agent');
    }}>Use returned program</Button>
    <Button type="submit">Create profile</Button>
    <Button type="reset">Reset profile</Button>
  </form>;
}

it('retains a newly added selection in a form, submits it, and still permits an actual choice and reset', async () => {
  const changed = vi.fn<(value: string) => void>();
  const submitted = vi.fn<(value: string) => void>();
  const user = userEvent.setup();
  show(<StrictMode><Profile changed={changed} submitted={submitted} /></StrictMode>);
  const picker = screen.getByRole('combobox', { name: 'Agent program' });
  expect(picker).toHaveTextContent('Choose a program');
  await user.click(screen.getByRole('button', { name: 'Use returned program' }));
  await waitFor(() => expect(picker).toHaveTextContent('Research agent'));
  expect(changed).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Create profile' }));
  expect(submitted).toHaveBeenCalledExactlyOnceWith('research-agent');

  await user.click(picker);
  await user.click(screen.getByRole('option', { name: 'Codex' }));
  expect(picker).toHaveTextContent('Codex');
  expect(changed).toHaveBeenCalledExactlyOnceWith('codex');

  await user.click(screen.getByRole('button', { name: 'Reset profile' }));
  await waitFor(() => expect(picker).toHaveTextContent('Choose a program'));
  expect(changed).toHaveBeenLastCalledWith('');
  expect(changed).toHaveBeenCalledTimes(2);
  expect(submitted).toHaveBeenCalledTimes(1);
});
