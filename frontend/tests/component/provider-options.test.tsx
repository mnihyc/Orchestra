import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import { RequestOptions } from '@/features/models/RequestOptions';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const schema = { type: 'object', additionalProperties: false, properties: {
  mode: { title: 'Mode', type: 'string', enum: ['a', 'b'], default: 'a' },
  region: { title: 'Region', const: 'local', 'x-orch-source': 'provider' },
  unsupported: false,
} };

it.each(['en', 'zh-CN'] as const)('renders allowed choices and provider-fixed values in %s', async (locale) => {
  await setLocale(locale);
  function Form() {
    const [value, setValue] = useState<unknown>({ mode: 'a' });
    return <RequestOptions schema={schema} value={value} onChange={setValue} />;
  }
  show(<Form />);
  expect(screen.getByRole('combobox', { name: 'Mode' })).not.toBeVisible();
  const user = userEvent.setup();
  const heading = screen.getByText(i18n.t('providerOptions.title'), { selector: 'summary span' });
  await user.click(heading);
  expect(screen.getByRole('textbox', { name: 'Region' })).toBeDisabled();
  expect(screen.getByRole('textbox', { name: 'Region' })).toHaveValue('local');
  expect(screen.getByText(locale === 'en' ? 'Fixed by the provider' : '由提供方固定')).toBeVisible();
  expect(screen.queryByLabelText('unsupported')).not.toBeInTheDocument();
  await user.click(screen.getByRole('combobox', { name: 'Mode' }));
  expect(screen.getAllByRole('option')).toHaveLength(2);
  await user.click(screen.getByRole('option', { name: 'b' }));
  expect(screen.getByRole('combobox', { name: 'Mode' })).toHaveValue('b');
  await user.click(heading);
  expect(screen.getByRole('combobox', { name: 'Mode' })).not.toBeVisible();
  await user.click(heading);
  expect(screen.getByRole('combobox', { name: 'Mode' })).toHaveValue('b');
});

it('uses a dropdown for an allowed boolean subset without turning its default into a lock', async () => {
  await setLocale('en');
  const changed = vi.fn();
  show(<RequestOptions schema={{ type: 'object', additionalProperties: false,
    properties: { enabled: { type: 'boolean', title: 'Enabled', enum: [true], default: true } } }}
    value={{}} onChange={changed} />);
  await userEvent.click(screen.getByText(i18n.t('providerOptions.title'), { selector: 'summary span' }));
  expect(screen.queryByRole('switch')).not.toBeInTheDocument();
  const picker = screen.getByRole('combobox', { name: 'Enabled' });
  expect(picker).toBeEnabled();
  const user = userEvent.setup();
  await user.click(picker);
  const choices = screen.getAllByRole('option');
  expect(choices).toHaveLength(1);
  await user.click(choices[0]!);
  expect(changed).toHaveBeenCalledWith({ enabled: true });
});
