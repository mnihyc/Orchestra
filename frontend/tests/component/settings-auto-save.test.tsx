/** Automatic settings writes keep drafts, accepted values, native fences and one deliberate inverse. */
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import { AutoSaveFeedback, useAutoSave } from '@/features/settings/AutoSave';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { TYPING_PAUSE_MS } from '@/lib/debounce';
import { Input } from '@/ui/Input';
import { show } from './support';

interface Accepted { value: string; revision: number }
type Write = (value: string, revision: number) => Promise<Accepted>;
const appearance = useAppearance.getState();
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale('en'); });

function Name({ write, reload }: { write: Write; reload?: () => Promise<Accepted> }) {
  const [current, setCurrent] = useState<Accepted>({ value: 'Original', revision: 4 });
  const save = useAutoSave({ current, valid: (value) => value.trim() !== '',
    write: async (value, before) => {
      if (typeof before.revision !== 'number') throw new Error('A revision is required');
      const accepted = await write(value, before.revision); setCurrent(accepted); return accepted;
    }, reload: async () => { if (reload !== undefined) setCurrent(await reload()); } });
  return <form onSubmit={(event) => { event.preventDefault(); save.commit(); }}
    onCompositionStart={() => save.composing(true)} onCompositionEnd={() => save.composing(false)}>
    <Input label={i18n.t('settings.general.name')} value={save.value} onChange={(event) => save.edit(event.target.value)} disabled={save.reloading} />
    <AutoSaveFeedback save={save} />
    <button type="button" onClick={() => setCurrent({ value: 'Changed elsewhere', revision: 9 })}>Remote update</button>
    <output aria-label="Stored value">{current.value}</output>
  </form>;
}

for (const [locale, theme] of modes) it(`saves a valid edit and undoes only its accepted native revision in ${locale}/${theme}`, async () => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  const write = vi.fn<Write>().mockImplementation((value, revision) => Promise.resolve({ value: value.trim(), revision: revision + 1 }));
  const user = userEvent.setup(); show(<Name write={write} />);
  expect(write).not.toHaveBeenCalled();
  const field = screen.getByRole('textbox', { name: i18n.t('settings.general.name') });
  await user.clear(field); await user.type(field, 'New name');
  await waitFor(() => expect(screen.getByText(i18n.t('settings.compact.saved'), { exact: true })).toBeVisible());
  expect(write).toHaveBeenCalledExactlyOnceWith('New name', 4);
  expect(screen.getByLabelText('Stored value')).toHaveTextContent('New name');
  expect(screen.queryByRole('button', { name: i18n.t('settings.save') })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: i18n.t('settings.compact.undo') }));
  await waitFor(() => expect(field).toHaveValue('Original'));
  expect(write.mock.calls).toEqual([['New name', 4], ['Original', 5]]);
  expect(screen.getByText(i18n.t('settings.compact.undone'), { exact: true })).toBeVisible();
});

it('lets typing continue during a write and queues it against that answer without erasing newer text', async () => {
  let accept: (value: Accepted) => void = () => { throw new Error('The first write has not started'); };
  const write = vi.fn<Write>().mockImplementationOnce(() => new Promise((resolve) => { accept = resolve; }))
    .mockImplementation((value, revision) => Promise.resolve({ value, revision: revision + 1 }));
  const user = userEvent.setup(); show(<Name write={write} />);
  const field = screen.getByRole('textbox', { name: 'Name' });
  await user.clear(field); await user.type(field, 'First draft');
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  expect(field).toBeEnabled();
  await user.clear(field); await user.type(field, 'Later draft');
  expect(field).toHaveValue('Later draft'); expect(write).toHaveBeenCalledTimes(1);
  await act(async () => { accept({ value: 'First draft', revision: 5 }); await Promise.resolve(); });
  expect(field).toHaveValue('Later draft');
  await waitFor(() => expect(screen.getByLabelText('Stored value')).toHaveTextContent('Later draft'));
  expect(write.mock.calls).toEqual([['First draft', 4], ['Later draft', 5]]);
});

it('keeps an empty or composing name out of writes until it is a complete valid edit', async () => {
  const write = vi.fn<Write>().mockImplementation((value, revision) => Promise.resolve({ value, revision: revision + 1 }));
  const user = userEvent.setup(); show(<Name write={write} />);
  const field = screen.getByRole('textbox', { name: 'Name' });
  await user.clear(field);
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, TYPING_PAUSE_MS * 2)); });
  expect(write).not.toHaveBeenCalled();
  fireEvent.compositionStart(field); fireEvent.change(field, { target: { value: '新的名称' } });
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, TYPING_PAUSE_MS * 2)); });
  expect(write).not.toHaveBeenCalled(); expect(field).toHaveValue('新的名称');
  fireEvent.compositionEnd(field);
  await waitFor(() => expect(write).toHaveBeenCalledExactlyOnceWith('新的名称', 4));
});

it('keeps a refused draft and waits for explicit retry instead of a repeating write loop', async () => {
  const write = vi.fn<Write>().mockRejectedValueOnce(new Error('The name could not be saved.'))
    .mockImplementation((value, revision) => Promise.resolve({ value, revision: revision + 1 }));
  const user = userEvent.setup(); show(<Name write={write} />);
  const field = screen.getByRole('textbox', { name: 'Name' });
  await user.clear(field); await user.type(field, 'Keep this draft');
  await screen.findByRole('alert');
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, TYPING_PAUSE_MS * 2)); });
  expect(write).toHaveBeenCalledTimes(1); expect(field).toHaveValue('Keep this draft');
  await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  await waitFor(() => expect(screen.getByLabelText('Stored value')).toHaveTextContent('Keep this draft'));
  expect(write).toHaveBeenCalledTimes(2);
});

it('does not overwrite a later value with Undo or silently rebase an older draft', async () => {
  const write = vi.fn<Write>().mockImplementation((value, revision) => Promise.resolve({ value, revision: revision + 1 }));
  const user = userEvent.setup(); show(<Name write={write} />);
  const field = screen.getByRole('textbox', { name: 'Name' });
  await user.clear(field); await user.type(field, 'Accepted name');
  await screen.findByRole('button', { name: i18n.t('settings.compact.undo') });
  await user.click(screen.getByRole('button', { name: 'Remote update' }));
  expect(field).toHaveValue('Changed elsewhere');
  const undo = screen.getByRole('button', { name: i18n.t('settings.compact.undo') });
  expect(undo).toHaveAttribute('aria-disabled', 'true'); await user.click(undo);
  expect(write).toHaveBeenCalledTimes(1);
});

it('reads the latest value only after the person chooses recovery from a native stale-revision refusal', async () => {
  const write = vi.fn<Write>().mockRejectedValue(new ApiError({ type: 'orch:error:conflict.stale_revision', title: 'Changed', status: 409,
    code: 'conflict.stale_revision', detail: 'This value changed elsewhere.', request_id: 'settings-stale' }, null));
  const reload = vi.fn<() => Promise<Accepted>>().mockResolvedValue({ value: 'Changed elsewhere', revision: 9 });
  const user = userEvent.setup(); show(<Name write={write} reload={reload} />);
  const field = screen.getByRole('textbox', { name: 'Name' });
  await user.clear(field); await user.type(field, 'My draft');
  const latest = await screen.findByRole('button', { name: i18n.t('settings.auto.latest') });
  expect(field).toHaveValue('My draft'); expect(reload).not.toHaveBeenCalled();
  expect(screen.getAllByRole('alert')).toHaveLength(1);
  await user.click(latest);
  await waitFor(() => expect(field).toHaveValue('Changed elsewhere'));
  expect(reload).toHaveBeenCalledTimes(1); expect(write).toHaveBeenCalledTimes(1);
});
