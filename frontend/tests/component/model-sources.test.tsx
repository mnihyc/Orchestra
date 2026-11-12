/** Read-only model source cards keep actual vendor facts apart from catalogue proposals and configured choices. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { ModelSources } from '@/features/resources/providers/ModelSources';
import type { ModelSuggestion, ProviderModel } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { MODEL_ESTIMATE } from '../doubles/fixtures';
import { show } from './support';

const current: ProviderModel = { ...MODEL_ESTIMATE, id: 'source-model', model_name: 'source-model', display_name: 'My name',
  enabled: true, source: 'manual', groups: [], parameters: { input_images: true } };
const observed: ModelSuggestion = { model_name: current.model_name, display_name: 'Vendor name', listed: true,
  parameters: { input_images: false, offered: { context_max: 200000, web_search: ['live'] } },
  origins: { '/display_name': 'vendor', '/parameters/input_images': 'vendor',
    '/parameters/offered/context_max': 'vendor', '/parameters/offered/web_search': 'catalogue' },
};
let locale = currentLocale();
let appearance = useAppearance.getState();
beforeEach(() => { locale = currentLocale(); appearance = useAppearance.getState(); });
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale(locale); });

describe.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('model sources %s/%s', (language, theme) => {
  beforeEach(async () => { await setLocale(language); useAppearance.setState({ locale: language, theme, resolvedTheme: theme }); });

  it('opens the bounded actual field sources, names vendor differences and keeps configured values unchanged', async () => {
    const before = JSON.stringify([current, observed]);
    show(<ModelSources observed={observed} current={current} at="2026-11-08T06:00:00Z" />);
    expect(screen.getByText(i18n.t('providerFill.vendorDiffers', { fields: `${i18n.t('providerFill.fields.name')}, ${i18n.t('providerFill.fields.images')}` }))).toBeVisible();
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: i18n.t('providerFill.reportedValues') }));
    const card = screen.getByRole('dialog', { name: i18n.t('providerFill.reportedValues') });
    const image = card.querySelector('[data-observed-field="images"]');
    if (image === null) throw new Error('the image field must have its own source row');
    expect(within(image as HTMLElement).getByText(i18n.t('providerFill.no'), { exact: true })).toBeVisible();
    expect(within(image as HTMLElement).getByText(i18n.t('providerFill.sources.vendor'), { exact: true })).toBeVisible();
    const search = card.querySelector('[data-observed-field="search"]');
    if (search === null) throw new Error('the catalogue offer must have its own source row');
    expect(search).toHaveTextContent(i18n.t('providerFill.sources.catalogue'));
    expect(card.querySelector('time')).toHaveAttribute('dateTime', '2026-11-08T06:00:00Z');
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(JSON.stringify([current, observed])).toBe(before);
  });

  it('names missing confirmation and never borrows another observation time', async () => {
    const rendered = show(<ModelSources observed={{ ...observed, origins: undefined }} current={current} at={null} />);
    expect(screen.getByText(i18n.t('providerFill.unconfirmed'))).toBeVisible();
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
    rendered.rerender(<ModelSources observed={observed} current={current} at={null} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('providerFill.reportedValues') }));
    expect(screen.getByText(i18n.t('providerFill.timeUnknown'))).toBeVisible();
    expect(screen.getByRole('dialog').querySelector('time')).not.toBeInTheDocument();
  });
});
