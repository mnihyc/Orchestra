/** A returned program can add and select an option in one render without a native mirror clearing it. */
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { StrictMode, useState } from 'react';
import { expect, it, vi } from 'vitest';

import { Button } from '@/ui/Button';
import { client } from '@/api/client';
import { requestReader } from '@/lib/readAttempts';
import { bindDeviceChoice } from '@/lib/viewChoiceFacts';
import { FactDisclosureBody } from '@/ui/FactDisclosure';
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


it('offers closed keyboard choices before mounting the details of an unopened list', async () => {
  const changed = vi.fn();
  const Facts = vi.fn(() => <p>Program facts</p>);
  const user = userEvent.setup();
  show(<Select label="Program" value="codex" onValueChange={changed} footer={<Facts />}
    options={[{ value: 'codex', label: 'Codex' }, { value: 'research', label: 'Research' }]} />);
  const trigger = screen.getByRole('combobox', { name: 'Program' });
  await waitFor(() => expect(trigger).toHaveTextContent('Codex'));
  act(() => trigger.focus());
  await user.keyboard('r');
  expect(changed).toHaveBeenCalledWith('research');
  expect(Facts).not.toHaveBeenCalled();
  await user.click(trigger);
  expect(await screen.findByText('Program facts')).toBeVisible();
});

it('keeps domain facts and device facts in the same selector without choosing while reading them', async () => {
  const original = client.session.getState();
  act(() => client.session.setState({ status: 'signed_in', userId: 'choice-reader', readerVersion: 1 }));
  const changed = bindDeviceChoice(vi.fn<(value: string) => void>(), {
    reader: requestReader('choice-reader', 1), scope: { kind: 'page', key: 'view', thing: 'history-choice' },
    chosen: 'codex', effective: 'codex', saved: 'codex', origin: 'item',
    saved_choice_availability: 'available', read_result: 'available', write_result: 'not_checked',
  });
  try {
    const user = userEvent.setup();
    show(<Select label="Agent program" value="codex" onValueChange={changed}
      options={[{ value: 'codex', label: 'Codex' }, { value: 'research', label: 'Research' }]}
      footer={<FactDisclosureBody groups={[{ key: 'history', facts: [{ key: 'coverage', label: 'Proposed history', value: 'Complete transcript' }] }]}>
        <a href="#history-source">History source</a><button type="button">Recheck history</button>
      </FactDisclosureBody>} />);
    const trigger = screen.getByRole('combobox', { name: 'Agent program' });
    await user.click(trigger);
    const popup = await screen.findByRole('dialog', { name: 'Agent program' });
    const list = within(popup).getByRole('listbox', { name: 'Agent program' });
    expect(popup.querySelectorAll('[data-fact-disclosure-body]')).toHaveLength(2);
    expect(within(popup).getByText('Complete transcript')).toBeVisible();
    const source = within(popup).getByRole('link', { name: 'History source' });
    expect(list).not.toContainElement(source);
    await waitFor(() => expect(within(list).getByRole('option', { name: 'Codex' })).toHaveFocus());
    await user.tab(); expect(source).toHaveFocus();
    await user.tab(); expect(within(popup).getByRole('button', { name: 'Recheck history' })).toHaveFocus();
    expect(changed).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: 'Details' })).not.toBeInTheDocument();
    await user.keyboard('{Escape}');
    expect(trigger).toHaveFocus(); expect(trigger).toHaveTextContent('Codex');
    expect(changed).not.toHaveBeenCalled();
  } finally { act(() => client.session.setState(original)); }
});
