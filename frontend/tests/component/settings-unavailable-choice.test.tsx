/** Known unsupported schema choices stay visible without joining the offered/valid values. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { SettingChoiceContext } from '@/features/settings/choiceCopy';
import { ValueControl } from '@/features/settings/ValueControl';
import { availableShape, emptyOf, shapeOf } from '@/features/settings/settingsModel';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const LABEL = { en: 'Approval mode', 'zh-CN': '审批模式' } as const;
const AUTO = 'program_auto_unavailable';
const unavailableSchema = (reason = AUTO) => ({ type: 'string', enum: ['ask', 'on_request', 'bypass'],
  'x-unavailable-choices': [{ value: 'auto', reason }] });
function Form({ schema, start, onChange, label, setting = 'approval.mode', disabled = false }: {
  schema: unknown; start: unknown; onChange: (value: unknown) => void; label: string; setting?: string; disabled?: boolean;
}) {
  const [value, setValue] = useState(start);
  return <SettingChoiceContext.Provider value={setting}><ValueControl shape={shapeOf(schema)} value={value} label={label} disabled={disabled}
    onChange={(next) => { onChange(next); setValue(next); }} /></SettingChoiceContext.Provider>;
}
afterEach(async () => { cleanup(); await setLocale('en'); });

// EN: Settings → Approval mode → open the choice list → Auto remains disabled with its program reason.
// 中文：设置 → 审批模式 → 展开选项 → 自动仍然显示，并因程序不支持而禁用。
for (const locale of ['en', 'zh-CN'] as const) it(`keeps Auto visible with the declared program-support reason and never selects it (${locale})`, async () => {
  await setLocale(locale);
  const changed = vi.fn<(value: unknown) => void>();
  show(<Form schema={unavailableSchema()} start="ask" label={LABEL[locale]} onChange={changed} />);
  const user = userEvent.setup();
  await user.click(screen.getByRole('combobox', { name: LABEL[locale] }));
  const automatic = screen.getByRole('option', { name: i18n.t('choiceCatalog.approvalMode.auto.name') });
  expect(automatic).toHaveAttribute('aria-disabled', 'true');
  expect(screen.getByText(i18n.t('settings.editor.autoUnavailable'))).toBeVisible();
  await user.click(automatic);
  expect(changed).not.toHaveBeenCalled();
  const shape = availableShape(shapeOf(unavailableSchema()), ['ask', 'on_request']);
  if (shape.type !== 'choice') throw new Error('Expected the declared scalar choice');
  expect(shape.values).toEqual(['ask', 'on_request']);
  expect(emptyOf(shape)).toBe('ask');
});

for (const locale of ['en', 'zh-CN'] as const) it(`keeps a retained Auto unchanged and labelled while leaving it unavailable (${locale})`, async () => {
  await setLocale(locale);
  const changed = vi.fn<(value: unknown) => void>();
  show(<Form schema={unavailableSchema()} start="auto" label={LABEL[locale]} onChange={changed} />);
  const field = screen.getByRole('combobox', { name: LABEL[locale] });
  expect(field).toHaveValue(i18n.t('choiceCatalog.approvalMode.auto.name'));
  expect(changed).not.toHaveBeenCalled();
  await userEvent.setup().click(field);
  expect(screen.getByRole('option', { name: i18n.t('choiceCatalog.approvalMode.auto.name') })).toHaveAttribute('aria-disabled', 'true');
  expect(changed).not.toHaveBeenCalled();
});

for (const locale of ['en', 'zh-CN'] as const) it(`lets an existing unsupported set value be removed while preventing it from being added again (${locale})`, async () => {
  await setLocale(locale);
  const changed = vi.fn<(value: unknown) => void>();
  show(<Form schema={{ type: 'array', uniqueItems: true, items: unavailableSchema() }} start={['ask', 'auto']} setting="approval.allowed_modes" label={LABEL[locale]} onChange={changed} />);
  const automatic = screen.getByRole('checkbox', { name: i18n.t('choiceCatalog.approvalMode.auto.name') });
  expect(automatic).toBeChecked();
  expect(automatic).not.toBeDisabled();
  expect(screen.getByText(i18n.t('settings.editor.autoUnavailable'))).toBeVisible();
  await userEvent.setup().click(automatic);
  expect(changed).toHaveBeenLastCalledWith(['ask']);
  expect(automatic).not.toBeChecked();
  expect(automatic).toBeDisabled();
});

for (const locale of ['en', 'zh-CN'] as const) it(`keeps supported Auto selectable and does not print unknown private reason codes (${locale})`, async () => {
  await setLocale(locale);
  const changed = vi.fn<(value: unknown) => void>();
  const view = show(<Form schema={unavailableSchema('private-program-reference')} start="ask" label={LABEL[locale]} onChange={changed} />);
  await userEvent.setup().click(screen.getByRole('combobox', { name: LABEL[locale] }));
  expect(screen.getByText(i18n.t('settings.editor.choiceUnavailable'))).toBeVisible();
  expect(document.body).not.toHaveTextContent('private-program-reference');
  view.unmount();
  show(<Form schema={{ type: 'string', enum: ['ask', 'on_request', 'auto', 'bypass'] }} start="ask" label={LABEL[locale]} onChange={changed} />);
  const user = userEvent.setup();
  await user.click(screen.getByRole('combobox', { name: LABEL[locale] }));
  await user.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.approvalMode.auto.name') }));
  expect(changed).toHaveBeenLastCalledWith('auto');
});
