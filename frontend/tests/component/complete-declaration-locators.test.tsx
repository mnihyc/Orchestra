/** Declaration renderers keep their actual raw values and confirmation boundaries while locating a deep choice. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import { ElicitationForm } from '@/features/approvals/Harness';
import { SchemaInput } from '@/features/resources/templates/SchemaForm';
import type { Schema } from '@/features/resources/templates/schema';
import type { Field } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { FieldForm, initialValues, type FormValues } from '@/ui/FieldForm';
import { show } from './support';

it('keeps a declared default, optional unset mapping, field help/error and raw selection', async () => {
  const user = userEvent.setup(), changed = vi.fn();
  const field: Field = { name: 'format', label: 'Format', type: 'choice', required: false, default: 'native-12',
    help: 'Choose the declared format.', max_bytes: null,
    choices: Array.from({ length: 12 }, (_, index) => ({ value: `native-${index + 1}`, label: `Format ${index + 1}` })) };
  function Form() {
    const [values, setValues] = useState<FormValues>(() => initialValues([field]));
    return <FieldForm fields={[field]} values={values} errors={{ format: 'The server rejected this format.' }}
      helpFor={() => <button type="button">Format guide</button>}
      onChange={(name, value) => { changed(name, value); setValues((before) => ({ ...before, [name]: value })); }} />;
  }
  show(<Form />);
  const input = screen.getByRole('combobox', { name: /^Format/ });
  expect(input).toHaveValue('Format 12');
  expect(input).toHaveAccessibleDescription('The server rejected this format.');
  expect(screen.getByRole('button', { name: 'Format guide' })).toBeVisible();
  await user.click(input);
  expect(screen.getAllByRole('option')).toHaveLength(13);
  await user.type(input, 'native-11');
  expect(changed).not.toHaveBeenCalled();
  await user.keyboard('{Enter}');
  expect(changed).toHaveBeenCalledExactlyOnceWith('format', 'native-11');
  await user.click(input);
  await user.click(screen.getByRole('option', { name: i18n.t('resources.fieldForm.notSet') }));
  expect(changed).toHaveBeenLastCalledWith('format', '');
  expect(input).toHaveValue(i18n.t('resources.fieldForm.notSet'));
});

it('keeps a required unknown declaration value readable and cannot edit a disabled form', async () => {
  const changed = vi.fn();
  const field: Field = { name: 'format', label: 'Format', type: 'choice', required: true, default: null,
    help: 'Keep the unrecognised value until a deliberate change.', max_bytes: null,
    choices: Array.from({ length: 12 }, (_, index) => ({ value: `native-${index + 1}`, label: `Format ${index + 1}` })) };
  show(<FieldForm fields={[field]} values={{ format: 'future-format' }} disabled onChange={changed} />);
  const input = screen.getByRole('combobox', { name: 'Format' });
  expect(input).toHaveValue('future-format');
  expect(input).toHaveAccessibleDescription(field.help ?? '');
  expect(input).toBeDisabled();
  await userEvent.click(input);
  expect(screen.queryByRole('option')).not.toBeInTheDocument();
  expect(changed).not.toHaveBeenCalled();
});

it('locates an object alternative without replacing its current shape until selection', async () => {
  const user = userEvent.setup(), changed = vi.fn();
  const schema: Schema = { title: 'Connection', oneOf: Array.from({ length: 12 }, (_, index) => ({
    type: 'object', title: `Shape ${index + 1}`, description: `Connection shape ${index + 1}.`, required: [`field${index + 1}`],
    properties: { [`field${index + 1}`]: { type: 'string', title: `Field ${index + 1}`, default: `default-${index + 1}` } },
  })) };
  function Form() {
    const [value, setValue] = useState<unknown>({ field1: 'kept' });
    return <SchemaInput root={schema} schema={schema} parts={['connection']} value={value} driver="sample" disabled={false} problems={[]}
      onChange={(next) => { changed(next); setValue(next); }} />;
  }
  show(<Form />);
  const input = screen.getByRole('combobox', { name: i18n.t('templates.editor.shape') });
  await user.click(input);
  expect(screen.getAllByRole('option')).toHaveLength(12);
  await user.type(input, 'Shape 12');
  expect(changed).not.toHaveBeenCalled();
  expect(screen.getByRole('textbox', { name: 'Field 1' })).toHaveValue('kept');
  expect(screen.getByRole('option', { name: 'Shape 12' })).toHaveAccessibleDescription('Connection shape 12.');
  await user.keyboard('{Enter}');
  expect(changed).toHaveBeenCalledExactlyOnceWith({ field12: 'default-12' });
  expect(screen.getByRole('textbox', { name: 'Field 12' })).toHaveValue('default-12');
});

it('keeps an unknown enum value, maps indices back to numeric/null/empty values and leaves reset explicit', async () => {
  const user = userEvent.setup(), changed = vi.fn();
  const schema: Schema = { title: 'Mode', enum: [null, '', ...Array.from({ length: 12 }, (_, index) => index + 1)], default: 1, description: 'Declared modes.' };
  function Form() {
    const [value, setValue] = useState<unknown>(99);
    return <SchemaInput root={schema} schema={schema} parts={['mode']} value={value} driver="sample" disabled={false} problems={[]}
      onChange={(next) => { changed(next); setValue(next); }} />;
  }
  show(<Form />);
  const input = screen.getByRole('combobox', { name: 'Mode' });
  expect(input).toHaveValue('99');
  await user.click(input);
  expect(screen.getAllByRole('option')).toHaveLength(14);
  await user.type(input, '12');
  await user.keyboard('{Escape}');
  expect(input).toHaveValue('99'); expect(changed).not.toHaveBeenCalled();
  await user.click(input); await user.type(input, '12'); await user.keyboard('{Enter}');
  expect(changed).toHaveBeenCalledExactlyOnceWith(12);
  await user.click(input); await user.click(screen.getByRole('option', { name: 'null' }));
  expect(changed).toHaveBeenLastCalledWith(null);
  await user.click(input); await user.click(screen.getByRole('option', { name: '""' }));
  expect(changed).toHaveBeenLastCalledWith('');
  expect(input).toHaveValue('""');
  await user.click(screen.getByRole('button', { name: i18n.t('templates.editor.resetField', { name: 'Mode' }) }));
  expect(changed).toHaveBeenLastCalledWith(undefined);
  expect(input).toHaveValue('');
  expect(input).toHaveAttribute('placeholder', i18n.t('templates.editor.default', { value: '1' }));
});

it('keeps elicitation values local until Accept and cannot submit through choice search', async () => {
  const user = userEvent.setup(), send = vi.fn();
  const schema = { type: 'object', required: ['count'], properties: { count: { title: 'Count', description: 'Number requested by the server.', type: 'integer', default: 12,
    enum: Array.from({ length: 12 }, (_, index) => index + 1) } } };
  show(<ElicitationForm schema={schema} message="Choose a count" disabled={false} pending={false} onSend={send} />);
  const input = screen.getByRole('combobox', { name: 'Count' });
  expect(input).toHaveValue('12');
  expect(input).toHaveAccessibleDescription('Number requested by the server.');
  await user.click(input); expect(screen.getAllByRole('option')).toHaveLength(12);
  await user.type(input, 'no match'); await user.keyboard('{Enter}{Escape}{Enter}');
  expect(send).not.toHaveBeenCalled(); expect(input).toHaveValue('12');
  await user.click(input); await user.type(input, '11'); await user.keyboard('{Enter}');
  expect(send).not.toHaveBeenCalled(); expect(input).toHaveValue('11');
  await user.click(screen.getByRole('button', { name: i18n.t('approvals.accept') }));
  expect(send).toHaveBeenCalledExactlyOnceWith('accept', { count: 11 });
});

it('requires an explicit first elicitation choice when no default was supplied and honors pending', async () => {
  const user = userEvent.setup(), send = vi.fn();
  const schema = { type: 'object', required: ['mode'], properties: { mode: { title: 'Mode', type: 'string',
    enum: Array.from({ length: 12 }, (_, index) => `mode-${index + 1}`) } } };
  const view = show(<ElicitationForm schema={schema} message="Choose a mode" disabled={false} pending={false} onSend={send} />);
  const input = screen.getByRole('combobox', { name: 'Mode' });
  expect(input).toHaveValue('mode-1');
  expect(screen.getByRole('button', { name: i18n.t('approvals.accept') })).toBeDisabled();
  await user.click(input); await user.click(screen.getByRole('option', { name: 'mode-1' }));
  expect(send).not.toHaveBeenCalled();
  expect(screen.getByRole('button', { name: i18n.t('approvals.accept') })).toBeEnabled();
  view.rerender(<ElicitationForm schema={schema} message="Choose a mode" disabled={false} pending onSend={send} />);
  expect(input).toBeDisabled();
  await user.click(input); expect(screen.queryByRole('option')).not.toBeInTheDocument();
  expect(send).not.toHaveBeenCalled();
});
