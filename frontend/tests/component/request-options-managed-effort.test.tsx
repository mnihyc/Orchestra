/** A dedicated model picker owns effort; the schema form keeps all other declared constraints and values. */
import { cleanup, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { RequestOptions } from '@/features/models/RequestOptions';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); });
it.each(['en', 'zh-CN'] as const)('does not duplicate effort controls or discard effort when another constrained field changes (%s)', async (locale) => {
  await setLocale(locale); const change = vi.fn();
  const schema = { type: 'object', additionalProperties: false, properties: {
    reasoning: { type: 'object', properties: { effort: { type: 'string', enum: ['low', 'high'], title: 'Effort' } } },
    temperature: { type: 'number', title: 'Temperature', minimum: 0, maximum: 1 },
  } };
  const before = JSON.stringify(schema);
  show(<RequestOptions label={i18n.t('providerOptions.title')} schema={schema} value={{ effort: 'high', temperature: 0.2 }} managedEffort onChange={change} />);
  await userEvent.click(screen.getByText(i18n.t('providerOptions.title'), { selector: 'summary span' }));
  expect(screen.queryByRole('combobox', { name: 'Effort' })).not.toBeInTheDocument();
  expect(screen.queryByRole('textbox', { name: 'Effort' })).not.toBeInTheDocument();
  const temperature = screen.getByRole('spinbutton', { name: 'Temperature' });
  expect(temperature).toHaveAttribute('min', '0'); expect(temperature).toHaveAttribute('max', '1');
  fireEvent.change(temperature, { target: { value: '0.5' } });
  expect(change).toHaveBeenCalledWith({ effort: 'high', temperature: 0.5 });
  expect(JSON.stringify(schema)).toBe(before);
});
