/** Empty views and failures share one compact sentence, one next act and safe bilingual recovery. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { NetworkError } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { errorText } from '@/ui/errorText';
import { show } from './support';

const appearance = useAppearance.getState();
const oldTheme = document.documentElement.dataset.theme;
const wasDark = document.documentElement.classList.contains('dark');
afterEach(async () => {
  cleanup(); useAppearance.setState(appearance);
  document.documentElement.classList.toggle('dark', wasDark);
  if (oldTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = oldTheme;
  await setLocale('en');
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`keeps one empty sentence and a keyboard action without a giant placeholder (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const t = i18n.getFixedT(locale), act = vi.fn(), title = t('mcpServers.empty');
    const view = show(<EmptyState compact title={title} description={title} titleAs="h2" action={<Button onClick={act}>{t('common.add')}</Button>} />);
    expect(screen.getAllByText(title)).toHaveLength(1);
    expect(screen.getByRole('heading', { level: 2, name: title })).toBeVisible();
    expect(view.container.querySelectorAll('[data-state-copy]')).toHaveLength(1);
    expect(screen.getAllByRole('button')).toHaveLength(1);
    await userEvent.tab(); await userEvent.keyboard('{Enter}'); expect(act).toHaveBeenCalledTimes(1);
    expect(view.container.querySelector('[data-state-line]')).not.toHaveClass('py-12');
  });

  it(`maps network and unknown API failures without codes or duplicate next actions (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const t = i18n.getFixedT(locale), retry = vi.fn(), recover = vi.fn();
    const view = show(<ErrorState error={new NetworkError('offline')} onRetry={retry} />);
    expect(screen.getByRole('alert')).toHaveTextContent(t('network.offline'));
    expect(screen.getAllByRole('button')).toHaveLength(1);
    await userEvent.click(screen.getByRole('button', { name: t('common.tryAgain') })); expect(retry).toHaveBeenCalledTimes(1);
    const future = new ApiError({ type: 'orch:error:internal', status: 503, code: 'internal', title: 'Internal', detail: 'private implementation data', request_id: 'private-request-id' }, null);
    // Real clients can receive a newer server's code before their generated union knows it.
    Object.assign(future.problem, { code: 'new_protocol_failure' });
    view.rerender(<ErrorState error={future} onRetry={retry} action={<Button onClick={recover}>{t('common.cancel')}</Button>} />);
    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent(t('errors.internal'));
    expect(alert).not.toHaveTextContent(/new_protocol_failure|private implementation data|private-request-id/);
    expect(screen.getAllByRole('button')).toHaveLength(1);
    expect(screen.queryByRole('button', { name: t('common.tryAgain') })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: t('common.cancel') })); expect(recover).toHaveBeenCalledTimes(1);
  });

  it(`opens the exact authorized host recovery and returns keyboard focus (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const t = i18n.getFixedT(locale);
    const error = new ApiError({ type: 'orch:error:plan.blocked', status: 409, code: 'plan.blocked', title: 'Blocked', detail: 'opaque', request_id: 'request',
      host_blockers: [{ host: { id: 'host-id', name: 'Workshop' }, online: false, offline_since: null, message: 'offline' }] }, null);
    show(<ErrorAlert error={error} />);
    const opener = screen.getByRole('button', { name: t('ui.error.review') });
    expect(screen.getAllByRole('button')).toHaveLength(1);
    expect(screen.queryByText('Workshop')).not.toBeInTheDocument();
    await userEvent.click(opener);
    const dialog = screen.getByRole('dialog');
    expect(within(dialog).getByText('Workshop')).toBeVisible();
    const hosts=within(dialog).getByRole('list',{name:t('planHosts.title')});
    const host=within(hosts).getByRole('listitem');
    expect(host).toBeVisible();
    expect(host).toHaveTextContent(t('planHosts.unknownSince'));
    expect(host).toHaveTextContent(t('planHosts.offline'));
    expect(dialog).not.toHaveTextContent(/host-id|plan.blocked|opaque/);
    await userEvent.keyboard('{Escape}'); expect(screen.queryByRole('dialog')).not.toBeInTheDocument(); expect(opener).toHaveFocus();
  });
}

it.each(['en', 'zh-CN'] as const)('keeps native failure reasons but never prints opaque failure identifiers (%s)', async (locale) => {
  await setLocale(locale); const t = i18n.getFixedT(locale);
  expect(errorText({ code: 'context_overflow', message: 'context_overflow' }, t, { locale }).message).toBe(t('ui.error.contextLimit'));
  expect(errorText({ code: 'future_native', message: 'future_native' }, t, { locale }).message).toBe(t('ui.error.operationFailed'));
  // Server-word policy: even readable unknown program diagnostics need a catalogued next act.
  expect(errorText({ code: 'future_native', message: 'The executable could not be found at the selected path.' }, t, { locale }).message).toBe(t('ui.error.operationFailed'));
  expect(errorText(new Error('private exception and stack'), t, { locale }).message).toBe(t('errors.page'));
});

it.each(['en', 'zh-CN'] as const)('keeps the server field explanation available without showing its protocol path (%s)', async (locale) => {
  await setLocale(locale); const t = i18n.getFixedT(locale);
  const error = new ApiError({ type: 'orch:error:request.invalid', status: 400, code: 'request.invalid', title: 'Invalid', detail: 'Invalid request', request_id: 'private-request',
    errors: [{ path: 'config.internal_field', message: 'Choose a directory inside an allowed root.' }] }, null);
  show(<ErrorAlert error={error} />);
  await userEvent.click(screen.getByRole('button', { name: t('ui.error.review') }));
  const dialog = screen.getByRole('dialog');
  expect(dialog).toHaveTextContent('Choose a directory inside an allowed root.');
  expect(dialog).not.toHaveTextContent(/config.internal_field|private-request|request.invalid/);
});


it('keeps a whole-page empty heading centred without repeating an identical supplied description', () => {
  show(<EmptyState title="No entries" description="No entries" titleAs="h2" action={<Button>Choose a place</Button>} />);
  expect(screen.getAllByText('No entries')).toHaveLength(1);
  expect(screen.getByRole('heading', { name: 'No entries' })).toBeVisible();
  expect(screen.getByRole('button', { name: 'Choose a place' })).toBeVisible();
});
