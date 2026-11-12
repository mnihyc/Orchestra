/** Documented controls and raw JSON share one authored draft, in both languages. */
import { useState } from 'react';
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { OptionSchemaEditor } from '@/features/models/OptionSchemaEditor';
import { EndpointHeaders } from '@/features/models/EndpointHeaders';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';
const changed = vi.fn<(value: string) => void>();
function Editor({ initial, disabled = false }: { initial: string; disabled?: boolean }) {
  const [value, setValue] = useState(initial);
  return <OptionSchemaEditor protocols={['openai-responses', 'anthropic-messages']} value={value} disabled={disabled} onChange={(next) => { changed(next); setValue(next); }} />;
}
afterEach(async () => { cleanup(); changed.mockClear(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('edits one request property and round-trips unrelated raw rules in %s', async (locale) => {
  await setLocale(locale);
  const original = { native: { properties: { effort: { const: 'high' } } }, vendor: { custom: 'keep' }, 'openai-responses': { type: 'object', properties: { untouched: { pattern: '^authored$' } } } };
  const view = show(<Editor initial={JSON.stringify(original)} />), user = userEvent.setup();
  const field = view.container.querySelector('[data-option-field="text.verbosity"]');
  if (!(field instanceof HTMLElement)) throw new Error('The documented verbosity field is visible');
  await user.click(within(field).getByRole('combobox', { name: i18n.t('providerFields.ruleFor', { name: i18n.t('providerFields.fields.verbosity') }) }));
  await user.click(screen.getByRole('option', { name: i18n.t('providerFields.constrain') }));
  await user.click(within(field).getByRole('switch', { name: i18n.t('providerFields.limitValues') }));
  const values = within(field).getByRole('textbox', { name: i18n.t('providerFields.valuesFor', { name: i18n.t('providerFields.fields.verbosity') }) });
  // One complete choice-list edit keeps this round-trip focused on the authored schema.
  await user.clear(values); await user.paste('low, medium');
  expect(values).toHaveValue('low, medium');
  const last = changed.mock.calls.at(-1)?.[0]; if (last === undefined) throw new Error('An edit reaches the existing draft');
  const parsed: unknown = JSON.parse(last);
  expect(parsed).toMatchObject({ native: original.native, vendor: original.vendor, 'openai-responses': { properties: { untouched: original['openai-responses'].properties.untouched, text: { properties: { verbosity: { enum: ['low', 'medium'] } } } } } });
  expect(last).not.toContain('default');
});

it.each(['en', 'zh-CN'] as const)('preserves malformed and custom drafts and disables viewer edits in %s', async (locale) => {
  await setLocale(locale); show(<Editor initial="{unfinished" />);
  expect(screen.getByRole('textbox', { name: i18n.t('providerFields.raw') })).toHaveValue('{unfinished');
  expect(screen.getByRole('alert')).toBeVisible(); expect(changed).not.toHaveBeenCalled();
  cleanup(); show(<Editor disabled initial={JSON.stringify({ 'openai-responses': { properties: { text: { $ref: '#/$defs/text' } } } })} />);
  expect(screen.getAllByRole('combobox').every((field) => field.hasAttribute('disabled'))).toBe(true);
  expect(changed).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('edits the existing case-insensitive public header map without losing custom headers in %s', async (locale) => {
  await setLocale(locale); const written = vi.fn<(value: Record<string, string>) => void>();
  function Headers() {
    const [value, setValue] = useState({ 'Anthropic-Beta': 'existing-beta', 'X-Custom': 'keep' });
    return <EndpointHeaders protocol="anthropic-messages" value={value} onChange={(next) => { written(next); setValue({ 'Anthropic-Beta': next['Anthropic-Beta'] ?? '', 'X-Custom': next['X-Custom'] ?? '' }); }} />;
  }
  show(<Headers />);
  const input = screen.getByRole('textbox', { name: i18n.t('providerFields.headerFor', { name: 'anthropic-beta' }) });
  await userEvent.clear(input); await userEvent.type(input, 'new-beta');
  expect(written).toHaveBeenCalledWith({ 'Anthropic-Beta': 'new-beta', 'X-Custom': 'keep' });
});
