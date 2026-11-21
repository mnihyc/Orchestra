/** Native and API diagnostics get one bilingual cause and a concrete recovery on screen. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, client } from '@/api/client';
import { beginReadAttempt, noteReadError, requestReader, retireReadAttempt, scheduleReadRetry } from '@/lib/readAttempts';
import type { Problem } from '@/generated/api-types';
import { UnitCommandFailure } from '@/features/resources/UnitConfiguration';
import { failureWords } from '@/features/export/jobs';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { formatSpan } from '@/lib/time';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { show } from './support';

const appearance = useAppearance.getState();
afterEach(async () => { cleanup();useAppearance.setState(appearance);await setLocale('en'); });
const diagnostic = 'harness_unavailable: agent.harnesses: model_inputs_segment_id_fkey threads_starting_step_check';
for (const locale of ['en','zh-CN'] as const) for (const theme of ['light','dark'] as const) {
  it(`uses catalogued recovery for native, job and API failures in ${locale}/${theme}`, async () => {
    await setLocale(locale);useAppearance.setState({ locale,theme,resolvedTheme:theme });
    const view = show(<ErrorAlert error={{ code:'harness_unavailable',message:diagnostic }} />);
    expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('ui.error.agentProgramUnavailable'));
    expect(view.container).not.toHaveTextContent(/harness_unavailable|agent\.harnesses|_fkey|_check/u);
    expect(failureWords({ code:'harness_unavailable',message:diagnostic,phase:'starting',undone:false },i18n.t)).toBe(i18n.t('ui.error.agentProgramUnavailable'));
    view.rerender(<ErrorAlert error={{ code:'history_import_failed',message:'the bridge refused the imported history' }} />);
    expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('ui.error.historyImportFailed'));
    expect(screen.getByRole('alert')).not.toHaveTextContent(i18n.t('ui.error.agentProgramUnavailable'));
    expect(failureWords({ code:'history_import_failed',message:'import refused',phase:'history',undone:true },i18n.t)).toBe(i18n.t('ui.error.historyImportFailed'));
    view.rerender(<ErrorAlert error={new ApiError({ type:'orch:error:conflict.exists',code:'conflict.exists',status:409,title:diagnostic,detail:diagnostic,request_id:'request' },null)} />);
    expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('errors.conflict.exists'));
    expect(view.container).not.toHaveTextContent(/_fkey|_check/u);
    const commandDiagnostic = `after-start command build failed: ${diagnostic}`;
    view.rerender(<UnitCommandFailure detail={commandDiagnostic} href="#commands" />);
    expect(screen.getByText(i18n.t('ui.error.afterStart'),{exact:true})).toBeVisible();
    expect(screen.getByRole('link',{name:i18n.t('unitConfiguration.changeCommands')})).toHaveAttribute('href','#commands');
    // Command output is opt-in; the ordinary screen and its recovery stay catalogued.
    const recovery = screen.getByRole('alert');
    expect(recovery).not.toHaveTextContent(/harness_unavailable|agent\.harnesses|_fkey|_check/u);
    expect(view.container).not.toHaveTextContent(/harness_unavailable|agent\.harnesses|_fkey|_check/u);
    expect(screen.queryByText(commandDiagnostic, { selector: 'pre', exact: true })).not.toBeInTheDocument();
    const user = userEvent.setup();
    await user.click(screen.getByText(i18n.t('unitConfiguration.commandOutput'), { selector: 'summary', exact: true }));
    const output = await screen.findByText(commandDiagnostic, { selector: 'pre', exact: true });
    expect(output).toBeVisible();
    expect(recovery).not.toContainElement(output);
    expect(recovery).not.toHaveTextContent(/harness_unavailable|agent\.harnesses|_fkey|_check/u);
    await user.click(screen.getByText(i18n.t('unitConfiguration.commandOutput'), { selector: 'summary', exact: true }));
    await waitFor(() => expect(screen.queryByText(commandDiagnostic, { selector: 'pre', exact: true })).not.toBeInTheDocument());
  });
}

const databasePhases = [
  ['acquire', 'Database connection wait expired', '等待数据库连接已超时'],
  ['statement', 'Database statement timed out', '数据库语句执行超时'],
  ['idle_transaction', 'Database transaction expired', '数据库事务已超时'],
] as const;

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const [phase, english, chinese] of databasePhases) {
  it(`keeps a ${phase} failure's captured deadline until a new request replaces it (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme, resolvedTheme: theme });
    const fetch = vi.spyOn(globalThis, 'fetch');
    try {
      const captured = new ApiError({ type: 'orch:error:internal', code: 'internal', status: 500,
        title: 'Request failed', detail: 'Do not inspect SQL here', request_id: 'earlier-database-request',
        database_wait: { phase, applied_limit_ms: 22_000, service: 'api', occurrence_id: 'earlier-database-request' },
      }, null);
      const page = show(<ErrorAlert error={captured} />);
      expect(screen.getByRole('alert')).toHaveTextContent(locale === 'en' ? english : chinese);
      const duration = formatSpan(22_000, { locale });
      expect(screen.queryByText(duration, { exact: true })).not.toBeInTheDocument();
      const button = screen.getByRole('button', { name: i18n.t('common.details') });
      button.focus(); await userEvent.keyboard('{Enter}');
      const facts = await screen.findByRole('dialog', { name: i18n.t('common.details') });
      expect(facts).toHaveTextContent(duration);
      expect(facts).toHaveTextContent(locale === 'en' ? 'Applied deadline' : '实际期限');
      expect(facts).not.toHaveTextContent(/earlier-database-request|Do not inspect SQL|idle_transaction/u);
      expect(fetch).not.toHaveBeenCalled();
      await userEvent.keyboard('{Escape}');
      await waitFor(() => expect(button).toHaveFocus());
      await userEvent.click(button);
      page.rerender(<ErrorAlert error={new ApiError({ type: 'orch:error:internal', code: 'internal', status: 500,
        title: 'Request failed', detail: 'Unknown database cause', request_id: 'later-request' }, null)} />);
      expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('errors.internal'));
      await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
      expect(screen.queryByRole('button', { name: i18n.t('common.details') })).not.toBeInTheDocument();
      expect(screen.queryByText(duration, { exact: true })).not.toBeInTheDocument();
    } finally { fetch.mockRestore(); }
  });
}

for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps an unreported deadline distinct from zero or no deadline (${locale})`, async () => {
    await setLocale(locale); useAppearance.setState({ locale });
    show(<ErrorAlert error={new ApiError({ type: 'orch:error:internal', code: 'internal', status: 500,
      title: 'Request failed', detail: 'Native timeout without a pool profile', request_id: 'unreported-deadline',
      database_wait: { phase: 'statement', applied_limit_ms: null, service: 'api', occurrence_id: 'unreported-deadline' },
    }, null)} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
    const facts = await screen.findByRole('dialog', { name: i18n.t('common.details') });
    expect(facts).toHaveTextContent(locale === 'en' ? 'Not reported' : '尚未报告');
    expect(facts).not.toHaveTextContent(/0 s|0秒|No deadline|无截止时间/u);
  });

  for (const receipt of [
    { phase: 'acquire', applied_limit_ms: 22_000, service: 'api', occurrence_id: 'another-request' },
    { phase: 'not_a_native_phase', applied_limit_ms: 22_000, service: 'api', occurrence_id: 'current-request' },
    { phase: 'statement', applied_limit_ms: -1, service: 'api', occurrence_id: 'current-request' },
  ]) it(`ignores a stale or malformed database receipt ${receipt.phase}/${receipt.applied_limit_ms} (${locale})`, async () => {
    await setLocale(locale); useAppearance.setState({ locale });
    show(<ErrorAlert error={new ApiError({ type: 'orch:error:internal', code: 'internal', status: 500,
      title: 'Request failed', detail: 'Untrusted text is not the timeout cause', request_id: 'current-request', database_wait: receipt,
    } as unknown as Problem, null)} />);
    expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('errors.internal'));
    expect(screen.queryByRole('button', { name: i18n.t('common.details') })).not.toBeInTheDocument();
  });
}


for (const locale of ['en', 'zh-CN'] as const) it(`keeps native failure facts in the existing recovery opening (${locale})`, async () => {
  await setLocale(locale); useAppearance.setState({ locale });
  const error = new ApiError({ type: 'orch:error:internal', code: 'internal', status: 500,
    title: 'Request failed', detail: 'Native deadline', request_id: 'recovering-request',
    database_wait: { phase: 'statement', applied_limit_ms: 22_000, service: 'api', occurrence_id: 'recovering-request' },
  }, null);
  show(<ErrorState error={error} message="A contextual read failed" recovery={{ label: 'Recover source', content: <button>Use retained source</button> }} />);
  expect(screen.getByRole('alert')).toHaveTextContent(locale === 'en' ? 'Database statement timed out' : '数据库语句执行超时');
  expect(screen.queryByRole('button', { name: i18n.t('common.details') })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Recover source' }));
  const opening = await screen.findByRole('dialog', { name: 'Recover source' });
  expect(opening).toHaveTextContent(formatSpan(22_000, { locale }));
  expect(opening.querySelectorAll('[data-fact-disclosure-body]')).toHaveLength(1);
  expect(screen.getByRole('button', { name: 'Use retained source' })).toBeVisible();
  expect(screen.queryByRole('button', { name: i18n.t('common.details') })).not.toBeInTheDocument();
});


it('keeps one opening for a captured database failure and its current safe-read retry', async () => {
  await setLocale('en');
  const owner = {}, flight = {};
  const session = client.session.getState();
  const error = new ApiError({ type: 'orch:error:internal', code: 'internal', status: 500,
    title: 'Request failed', detail: 'Native deadline', request_id: 'merged-read',
    database_wait: { phase: 'statement', applied_limit_ms: 22_000, service: 'api', occurrence_id: 'merged-read' },
  }, null);
  beginReadAttempt(owner, flight, requestReader(session.userId, session.readerVersion));
  noteReadError(owner, flight, error);
  scheduleReadRetry(owner, error, 20_000);
  try {
    show(<ErrorAlert error={error} />);
    expect(screen.getAllByRole('button', { name: i18n.t('common.details') })).toHaveLength(1);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
    const opening = await screen.findByRole('dialog', { name: i18n.t('common.details') });
    expect(opening).toHaveTextContent(i18n.t('databaseWait.deadline'));
    expect(opening).toHaveTextContent(i18n.t('clientFacts.readAttempt'));
    expect(opening).toHaveTextContent(formatSpan(22_000, { locale: 'en' }));
    act(() => retireReadAttempt(owner, flight));
    expect(opening).not.toHaveTextContent(i18n.t('clientFacts.readAttempt'));
    expect(opening).toHaveTextContent(i18n.t('databaseWait.deadline'));
  } finally { retireReadAttempt(owner, flight); }
});
