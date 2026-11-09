/** Thread rows and controlled API choices share effort controls without a fabricated thread or writer. */
import { cleanup, fireEvent, screen, within } from '@testing-library/react';
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
  if (configured === undefined || configured.model === null || configured.provider === null) throw new Error('The model fixture needs an allowed API choice.');
  const option = configured.model, provider = configured.provider;
  return { id: 'first', value: { provider_id: provider.id, model: option.model_name, params: { reasoning_effort: 'low', temperature: 0.2 } },
    option: { ...option, parameters: { ...option.parameters, efforts: ['low', 'high'], default_effort: 'low' } }, providerName: provider.name };
}
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps Default first and shows its resolved preset independently of a personal choice (${locale})`, async () => {
    await setLocale(locale); const first = entry(), change = vi.fn(), t = i18n.getFixedT(locale);
    const value = { ...first.value, params: { effort: 'high' } };
    show(<ModelEffort open choice={{ label: t('composer.model.title'), value, entries: [first], onChange: change,
      defaultChoice: { label: t('settings.editor.from.default'), value: first.value } }} />);
    const rows = screen.getAllByRole('menuitemradio'), defaultRow = rows[0];
    expect(defaultRow).toHaveTextContent(t('settings.editor.from.default'));
    expect(defaultRow?.closest('li')).toHaveTextContent(first.option.display_name);
    expect(defaultRow?.closest('li')).toHaveTextContent(effortDisplayName('low', t));
    expect(defaultRow?.closest('li')).not.toHaveTextContent(effortDisplayName('high', t));
    expect(screen.getByRole('button', { name: t('composer.model.title') })).toHaveTextContent(effortDisplayName('high', t));
    if (defaultRow === undefined) throw new Error('The default must be the first row.');
    fireEvent.click(defaultRow); expect(change).toHaveBeenCalledWith(null);
  });

  it(`chooses a declared effort and preserves other preset members (${locale})`, async () => {
    await setLocale(locale); const first = entry(), change = vi.fn(), t = i18n.getFixedT(locale);
    show(<ModelEffort open choice={{ label: t('composer.model.title'), value: first.value, entries: [first], onChange: change }} />);
    const group = screen.getByRole('group', { name: t('composer.model.efforts', { model: first.option.display_name }) });
    fireEvent.click(within(group).getByRole('menuitemradio', { name: effortDisplayName('high', t) }));
    expect(change).toHaveBeenCalledWith({ ...first.value, params: { effort: 'high', temperature: 0.2 } }, first);
  });

  it(`shows the entry preset when the personal override does not choose effort (${locale})`, async () => {
    await setLocale(locale); const first = entry(), t = i18n.getFixedT(locale);
    const preset = { ...first, option: { ...first.option, parameters: { ...first.option.parameters, default_effort: 'high' } } };
    show(<ModelEffort choice={{ label: t('composer.model.title'), value: { provider_id: first.value.provider_id, model: first.value.model, params: { temperature: 0.5 } },
      entries: [preset], onChange: () => undefined }} />);
    expect(screen.getByRole('button', { name: t('composer.model.title') })).toHaveTextContent(effortDisplayName('low', t));
    expect(screen.getByRole('button', { name: t('composer.model.title') })).not.toHaveTextContent(effortDisplayName('high', t));
  });

  it(`offers the model without inventing an effort when none is declared (${locale})`, async () => {
    await setLocale(locale); const first = entry(), change = vi.fn(), t = i18n.getFixedT(locale);
    const plain = { ...first, value: { provider_id: first.value.provider_id, model: first.value.model },
      option: { ...first.option, parameters: { ...first.option.parameters, efforts: [], default_effort: null } } };
    show(<ModelEffort open choice={{ label: t('composer.model.title'), value: null, entries: [plain], onChange: change }} />);
    expect(screen.queryByRole('group', { name: t('composer.model.efforts', { model: plain.option.display_name }) })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('menuitemradio', { name: plain.option.display_name }));
    expect(change).toHaveBeenCalledWith(plain.value, plain);
  });

  it(`keeps a refused model's native reason and never chooses it (${locale})`, async () => {
    await setLocale(locale); const first = entry(), change = vi.fn(), t = i18n.getFixedT(locale);
    const reason = t('ask.modelExclusion_no_use');
    const refused = { ...first, reason, option: { ...first.option, allowed: false, reason: 'excluded_by_entry' as const } };
    show(<ModelEffort open choice={{ label: t('composer.model.title'), value: null, entries: [refused], onChange: change }} />);
    const row = screen.getByRole('menuitemradio', { name: refused.option.display_name });
    expect(row).toBeDisabled(); expect(row.closest('li')).toHaveTextContent(reason);
    fireEvent.click(row); expect(change).not.toHaveBeenCalled();
  });

  it(`hands declared option edits back without losing effort or bypassing a disabled control (${locale})`, async () => {
    await setLocale(locale); const first = entry(), change = vi.fn(), t = i18n.getFixedT(locale);
    const choice = { label: t('composer.model.title'), value: first.value, entries: [first], onChange: change,
      renderParameters: (_entry: ModelEffortEntry, value: typeof first.value, onChange: (params: unknown) => void, disabled: boolean) =>
        <RequestOptions label="Temperature" schema={{ type: 'number', minimum: 0, maximum: 1 }} value={record(value.params).temperature} disabled={disabled}
          onChange={(temperature) => onChange({ ...record(value.params), temperature })} /> };
    const view = show(<ModelEffort open choice={choice} />);
    fireEvent.change(screen.getByRole('spinbutton', { name: 'Temperature' }), { target: { value: '0.5' } });
    expect(change).toHaveBeenCalledWith({ ...first.value, params: { reasoning_effort: 'low', temperature: 0.5 } }, first);
    view.rerender(<ModelEffort open choice={{ ...choice, disabled: true }} />);
    expect(screen.getByRole('spinbutton', { name: 'Temperature' })).toBeDisabled();
  });
}
