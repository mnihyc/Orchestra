/** Thread rows and controlled API choices share the model and effort controls without a fabricated thread or writer. */
import { cleanup, fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ModelEffort, type ModelEffortEntry } from '@/features/composer/ModelEffort';
import { effortDisplayName } from '@/features/models/names';
import { RequestOptions } from '@/features/models/RequestOptions';
import { record } from '@/features/resources/UnitCommands';
import { i18n, setLocale } from '@/i18n';
import { askModels } from '../unit/ask-fixtures';
import { show } from './support';

function entry(): ModelEffortEntry {
  const configured = askModels().models[0];
  if (!configured?.model || configured.provider === null) throw new Error('The model fixture needs an allowed API choice.');
  const option = configured.model, provider = configured.provider;
  return { id: 'first', value: { provider_id: provider.id, model: option.model_name, params: { reasoning_effort: 'low', temperature: 0.2 } },
    option: { ...option, parameters: { ...option.parameters, efforts: ['low', 'high'], default_effort: 'low' } }, providerName: provider.name };
}
const effortChip = (t: ReturnType<typeof i18n.getFixedT>, value: string) => screen.getByRole('button', { name: t('composer.chip.value', { what: t('composer.effort.title'), value: effortDisplayName(value, t) }) });
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps Default first in the model list and shows the effort of a personal choice on its own control (${locale})`, async () => {
    await setLocale(locale); const first = entry(), change = vi.fn(), t = i18n.getFixedT(locale);
    const value = { ...first.value, params: { effort: 'high' } };
    show(<ModelEffort open choice={{ label: t('composer.model.chip'), value, entries: [first], onChange: change,
      defaultChoice: { label: t('settings.editor.from.default'), value: first.value } }} />);
    const rows = screen.getAllByRole('menuitemradio'), defaultRow = rows[0];
    expect(defaultRow).toHaveTextContent(t('settings.editor.from.default'));
    expect(defaultRow?.closest('li')).toHaveTextContent(first.option.display_name);
    // Efforts are not nested under the models any more: the model list holds the default and the model.
    expect(rows).toHaveLength(2);
    expect(screen.queryByRole('menuitemradio', { name: effortDisplayName('low', t) })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: t('composer.model.chip') })).toHaveTextContent(first.option.display_name);
    expect(effortChip(t, 'high')).toHaveTextContent(effortDisplayName('high', t));
    if (defaultRow === undefined) throw new Error('The default must be the first row.');
    fireEvent.click(defaultRow); expect(change).toHaveBeenCalledWith(null);
  });

  it(`chooses a declared effort from the effort list and preserves other preset members (${locale})`, async () => {
    await setLocale(locale); const first = entry(), change = vi.fn(), t = i18n.getFixedT(locale);
    show(<ModelEffort choice={{ label: t('composer.model.chip'), value: first.value, entries: [first], onChange: change }} />);
    await userEvent.click(effortChip(t, 'low'));
    const list = await screen.findByRole('menu', { name: t('composer.effort.title') });
    expect(within(list).getAllByRole('menuitemradio')).toHaveLength(2);
    expect(within(list).getByRole('menuitemradio', { name: effortDisplayName('low', t), checked: true }).closest('li')).toHaveTextContent(t('composer.effort.default'));
    fireEvent.click(within(list).getByRole('menuitemradio', { name: effortDisplayName('high', t) }));
    expect(change).toHaveBeenCalledWith({ ...first.value, params: { effort: 'high', temperature: 0.2 } }, first);
  });

  it(`shows the entry preset when the personal override does not choose effort (${locale})`, async () => {
    await setLocale(locale); const first = entry(), t = i18n.getFixedT(locale);
    const preset = { ...first, option: { ...first.option, parameters: { ...first.option.parameters, default_effort: 'high' } } };
    show(<ModelEffort choice={{ label: t('composer.model.chip'), value: { provider_id: first.value.provider_id, model: first.value.model, params: { temperature: 0.5 } },
      entries: [preset], onChange: () => undefined }} />);
    expect(effortChip(t, 'low')).toHaveTextContent(effortDisplayName('low', t));
    expect(screen.queryByRole('button', { name: t('composer.chip.value', { what: t('composer.effort.title'), value: effortDisplayName('high', t) }) })).not.toBeInTheDocument();
  });

  it(`offers the model without inventing an effort when none is declared (${locale})`, async () => {
    await setLocale(locale); const first = entry(), change = vi.fn(), t = i18n.getFixedT(locale);
    const plain = { ...first, value: { provider_id: first.value.provider_id, model: first.value.model },
      option: { ...first.option, parameters: { ...first.option.parameters, efforts: [], default_effort: null } } };
    show(<ModelEffort open choice={{ label: t('composer.model.chip'), value: null, entries: [plain], onChange: change }} />);
    // The effort control is named "Effort: <value>"; the model control is named for the choice and may contain the word.
    expect(screen.queryByRole('button', { name: new RegExp(`^${t('composer.effort.title')}`) })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('menuitemradio', { name: plain.option.display_name }));
    expect(change).toHaveBeenCalledWith(plain.value, plain);
  });

  it(`keeps a refused model's native reason and never chooses it (${locale})`, async () => {
    await setLocale(locale); const first = entry(), change = vi.fn(), t = i18n.getFixedT(locale);
    const reason = t('ask.modelExclusion_no_use');
    const refused = { ...first, reason, option: { ...first.option, allowed: false, reason: 'excluded_by_entry' as const } };
    show(<ModelEffort open choice={{ label: t('composer.model.chip'), value: null, entries: [refused], onChange: change }} />);
    const row = screen.getByRole('menuitemradio', { name: refused.option.display_name });
    expect(row).toBeDisabled(); expect(row.closest('li')).toHaveTextContent(reason);
    fireEvent.click(row); expect(change).not.toHaveBeenCalled();
  });

  it(`hands declared option edits back without losing effort or bypassing a disabled control (${locale})`, async () => {
    await setLocale(locale); const first = entry(), change = vi.fn(), t = i18n.getFixedT(locale);
    const choice = { label: t('composer.model.chip'), value: first.value, entries: [first], onChange: change,
      renderParameters: (_entry: ModelEffortEntry, value: typeof first.value, onChange: (params: unknown) => void, disabled: boolean) =>
        <RequestOptions hideHeading label="Temperature" schema={{ type: 'number', minimum: 0, maximum: 1 }} value={record(value.params).temperature} disabled={disabled}
          onChange={(temperature) => onChange({ ...record(value.params), temperature })} /> };
    const view = show(<ModelEffort open choice={choice} />);
    fireEvent.change(screen.getByRole('spinbutton', { name: 'Temperature' }), { target: { value: '0.5' } });
    expect(change).toHaveBeenCalledWith({ ...first.value, params: { reasoning_effort: 'low', temperature: 0.5 } }, first);
    // A disabled choice cannot be opened, so its options cannot be reached to be edited.
    view.rerender(<ModelEffort open choice={{ ...choice, disabled: true }} />);
    expect(screen.queryByRole('spinbutton', { name: 'Temperature' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: t('composer.model.chip') })).toBeDisabled();
  });
}

it('searches a list of more than eight models and gives each model one row', async () => {
  const first = entry(), t = i18n.getFixedT('en'), user = userEvent.setup();
  const ten: ModelEffortEntry[] = Array.from({ length: 10 }, (_, at) => ({ ...first, id: `m${at}`, value: { ...first.value, model: `m${at}-routing`, params: undefined },
    option: { ...first.option, model_name: `m${at}-routing`, display_name: `Model ${at}`, parameters: { ...first.option.parameters, efforts: ['minimal', 'low', 'medium', 'high', 'xhigh', 'max'], default_effort: 'medium' } } }));
  show(<ModelEffort open choice={{ label: t('composer.model.chip'), value: null, entries: ten, onChange: () => undefined }} />);
  const menu = await screen.findByRole('menu', { name: t('composer.model.chip') });
  expect(within(menu).getAllByRole('menuitemradio')).toHaveLength(10);
  await user.type(screen.getByRole('textbox', { name: t('composer.model.find') }), 'Model 4');
  expect(within(menu).getAllByRole('menuitemradio')).toHaveLength(1);
});
