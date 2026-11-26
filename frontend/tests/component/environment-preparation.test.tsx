/** An actual environment producer owns its status; old and withdrawn facts do not persist. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it } from 'vitest';
import { EnvironmentPreparationFacts } from '@/features/thread/EnvironmentPreparationFacts';
import { i18n, setLocale } from '@/i18n';
import { detailEnvironment } from './resource-detail-fixtures';
import { show, skipOverflowFallback } from './support';
skipOverflowFallback();
afterEach(async () => { await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) it(`retires the actual attach facts when that preparation is gone (${locale})`, async () => {
  await setLocale(locale);
  const at = '2026-11-15T10:00:00.000Z';
  const value = { kind: 'attach' as const, environment: detailEnvironment, actual_job: null, actual_command: 'internal-command', observed_at: at };
  const view = show(<EnvironmentPreparationFacts stage={value} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  const body = await screen.findByRole('dialog', { name: i18n.t('common.details') });
  expect(body).toHaveTextContent(detailEnvironment.name);
  expect(body).toHaveTextContent(locale === 'en' ? 'Attaching directory' : '正在连接目录');
  expect(body).not.toHaveTextContent('internal-command');
  expect(body.querySelector(`time[datetime="${at}"]`)).not.toBeNull();
  view.rerender(<EnvironmentPreparationFacts stage={null} />);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});
