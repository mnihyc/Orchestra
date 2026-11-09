/** Reported support remains readable without turning it into a manager's permission or default. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it } from 'vitest';
import { ModelReport } from '@/features/resources/providers/ModelReport';
import { formOf, parametersOf } from '@/features/resources/providers/entry';
import { i18n, setLocale } from '@/i18n';
import { FUTURE_MODEL_REPORT, MODEL_REPORT } from '../fixtures/model-report';
import { show } from './support';

afterEach(async () => { cleanup(); delete document.documentElement.dataset.theme; await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`keeps prototype-named future values literal and readable (${locale}/${theme})`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    show(<ModelReport offered={FUTURE_MODEL_REPORT} source="Endpoint" />);
    await userEvent.click(screen.getByText(i18n.t('modelReport.title'), { exact: true }));
    const report = screen.getByTestId('model-report');
    expect(within(report).getAllByText('__proto__', { exact: true })).toHaveLength(2);
    expect(within(report).getByText('constructor', { exact: true })).toBeVisible();
    expect(within(report).getByText('toString', { exact: true })).toBeVisible();
    expect(within(report).getByText('constructor', { exact: true }).nextElementSibling).toHaveTextContent(i18n.t('modelReport.unsupported'));
    expect(report).not.toHaveTextContent('[object Object]');
    expect(report).not.toHaveTextContent('[native code]');
  });
  it(`reads the actual report and preserves it on a normal model edit (${locale}/${theme})`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    show(<ModelReport offered={MODEL_REPORT} source={i18n.t('models.editor.theEndpoint')} />);
    await userEvent.click(screen.getByText(i18n.t('modelReport.title'), { exact: true }));
    const report = screen.getByTestId('model-report');
    expect(report).toHaveAttribute('open');
    expect(within(report).getByText(i18n.t('modelReport.stages.deprecated'))).toBeVisible();
    expect(within(report).getByText('128,000')).toBeVisible();
    expect(within(report).getByText('1,000,000')).toBeVisible();
    expect(within(report).getByText('future-line')).toBeVisible();
    expect(report.querySelector('time[datetime="2027-01-01T00:00:00Z"]')).toBeInTheDocument();
    for (const name of ['pdf_input', 'thinking', 'structured_outputs', 'context_management', 'citations', 'batch']) {
      expect(within(report).getByText(i18n.t(`modelReport.capabilities.${name}`), { exact: true })).toBeVisible();
    }
    const falseRow = within(report).getByText(i18n.t('modelReport.capabilities.citations')).nextElementSibling;
    expect(falseRow).toHaveTextContent(i18n.t('modelReport.unsupported'));
    expect(report.querySelector('input,select,button')).toBeNull();
    const original = { input_images: true, offered: MODEL_REPORT };
    const saved = parametersOf(formOf(original), original);
    expect(saved.offered).toEqual(MODEL_REPORT);
    expect(saved.web_search).toBeUndefined();
    expect(saved.context_max).toBeUndefined();
    expect(saved.default_context).toBeUndefined();
  });
}

it('shows no invented report for an old endpoint and ignores malformed support and dates', () => {
  const { rerender } = show(<ModelReport offered={{ context_max: 200_000 }} source="Endpoint" />);
  expect(screen.queryByTestId('model-report')).not.toBeInTheDocument();
  rerender(<ModelReport offered={{ report: { capabilities: { image_input: { supported: 'true' } }, retires_at: 'invalid', output_max: -1 } }} source="Endpoint" />);
  expect(screen.queryByTestId('model-report')).not.toBeInTheDocument();
});
