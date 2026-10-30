/** Native and API diagnostics get one bilingual cause and a concrete recovery on screen. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it } from 'vitest';
import { ApiError } from '@/api/client';
import { UnitCommandFailure } from '@/features/resources/UnitConfiguration';
import { failureWords } from '@/features/export/jobs';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { ErrorAlert } from '@/ui/ErrorState';
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
