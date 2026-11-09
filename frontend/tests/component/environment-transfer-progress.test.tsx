/** Cleanup names an old unit only when that admitted fact exists; reopened work remains neutral. */
import { cleanup, screen, within } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { JobProgress } from '@/features/change/Progress';
import { transferFacts } from '@/features/change/environmentTransfer';
import { i18n, setLocale } from '@/i18n';
import { failedStart } from './failed-start-fixtures';
import { show } from './support';

afterEach(async () => { cleanup(); delete document.documentElement.dataset.theme; await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const unit of [false, true, null] as const) {
  it(`uses actual old-unit facts in activity and phase ticks ${locale}/${theme}/${String(unit)}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const base = failedStart().job;
    const job = { ...base, kind: 'environment_move', state: 'running' as const, started_at: base.created_at,
      progress: { ...base.progress, phase: 'cleanup', percent: 90, phases: [{ name: 'cleanup', state: 'running' as const }], message: 'The native transfer is finishing' } };
    const facts = transferFacts(unit === null ? undefined : { kind: 'switch_copy', source_has_unit: unit });
    const { container } = show(<JobProgress job={job} transfer={facts} heading={i18n.t('envTransfer.operation')} reconnecting={false} />);
    const text = i18n.t(unit === true ? 'envTransfer.cleanupUnit' : unit === false ? 'envTransfer.cleanupSource' : 'envTransfer.cleanup');
    expect(container.querySelector('[data-job-line]')).toHaveTextContent(text);
    const phases = screen.getByRole('list', { name: i18n.t('change.progress.phases') });
    expect(within(phases).getByRole('listitem')).toHaveTextContent(text);
    if (unit !== true) expect(container).not.toHaveTextContent(i18n.t('envTransfer.cleanupUnit'));
  });
}
