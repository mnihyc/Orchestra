/** A secret that is set is a filled field of dots with "Set" and Replace, never an empty box that looks as if nothing was saved. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import type { Field } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { FieldForm, type FormValues } from '@/ui/FieldForm';
import { SECRET_DOTS, SecretField } from '@/ui/SecretField';
import { show } from './support';

function Bare({ set, onRemove }: { set: boolean; onRemove?: () => void }) {
  const [value, setValue] = useState('');
  return <SecretField label="API key" value={value} onValueChange={setValue} set={set} {...(onRemove === undefined ? {} : { onRemove })} />;
}

it('shows a saved secret as dots with "Set" and a Replace act, and opens an empty hidden field only when asked', async () => {
  const user = userEvent.setup();
  show(<Bare set />);
  const dots = screen.getByLabelText('API key');
  expect(dots).toHaveValue(SECRET_DOTS);
  expect(dots).toHaveAttribute('readonly');
  expect(screen.getByText(i18n.t('settingsEditors.secret.set'))).toBeVisible();
  await user.click(screen.getByRole('button', { name: i18n.t('settingsEditors.secret.replace') }));
  const typed = screen.getByLabelText('API key');
  expect(typed).toHaveValue('');
  expect(typed).toHaveAttribute('type', 'password');
  expect(typed).toHaveFocus();
  await user.type(typed, 'sk-new');
  expect(typed).toHaveValue('sk-new');
  // Keeping the saved one puts the dots back and forgets what was typed.
  await user.click(screen.getByRole('button', { name: i18n.t('settingsEditors.secret.keep') }));
  expect(screen.getByLabelText('API key')).toHaveValue(SECRET_DOTS);
});

it('is an empty hidden field where nothing is set', () => {
  show(<Bare set={false} />);
  expect(screen.getByLabelText('API key')).toHaveValue('');
  expect(screen.queryByText(i18n.t('settingsEditors.secret.set'))).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: i18n.t('settingsEditors.secret.replace') })).not.toBeInTheDocument();
});

it('offers taking a saved secret away only where its field can', async () => {
  const user = userEvent.setup(), remove = vi.fn();
  const view = show(<Bare set />);
  expect(screen.queryByRole('button', { name: i18n.t('settingsEditors.secret.remove') })).not.toBeInTheDocument();
  view.unmount();
  show(<Bare set onRemove={remove} />);
  await user.click(screen.getByRole('button', { name: i18n.t('settingsEditors.secret.remove') }));
  expect(remove).toHaveBeenCalledOnce();
});

const adminKey: Field = { name: 'admin_key', label: 'Admin key', type: 'secret', required: false, help: null, choices: null, default: null, max_bytes: null };

it('draws the admin key of a declared form the same way: set, in dots, with Replace', async () => {
  const user = userEvent.setup(), changed = vi.fn();
  function Form({ set }: { set: boolean }) {
    const [values, setValues] = useState<FormValues>({ admin_key: '' });
    return <FieldForm fields={[adminKey]} values={values} secretsSet={set ? ['admin_key'] : []} onChange={(name, value) => { changed(name, value); setValues((before) => ({ ...before, [name]: value })); }} />;
  }
  const view = show(<Form set />);
  expect(screen.getByLabelText(/^Admin key/)).toHaveValue(SECRET_DOTS);
  expect(screen.getByText(i18n.t('settingsEditors.secret.set'))).toBeVisible();
  await user.click(screen.getByRole('button', { name: i18n.t('settingsEditors.secret.replace') }));
  await user.type(screen.getByLabelText(/^Admin key/), 'k');
  expect(changed).toHaveBeenLastCalledWith('admin_key', 'k');
  view.unmount();
  show(<Form set={false} />);
  expect(screen.getByLabelText(/^Admin key/)).toHaveValue('');
});
