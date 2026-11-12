/** A blocked first start has one truthful reason and real Retry/elsewhere actions. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { SourcePreparation } from '@/features/source/Preparation';
import { BlockedBanner } from '@/features/thread/BlockedBanner';
import type { Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { failedStart } from './failed-start-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ start: vi.fn<(id: string) => Promise<{ id: string }>>(), change: vi.fn() }));
vi.mock('@/api/environments', async (original) => ({ ...await original<object>(), startEnvironment: calls.start }));
vi.mock('@/features/change/target', async (original) => ({ ...await original<object>(), openChange: calls.change }));

function blocked(manager = true): Thread {
  const { thread } = failedStart();
  if (thread.environment === null) throw new Error('The failed-start fixture needs its actual environment');
  return { ...thread, id: 'thread', name: 'Login', binding: null,
    environment: { ...thread.environment, id: 'environment' }, environment_permissions: manager ? ['environment.manage'] : [],
    access: { ...thread.access, permissions: manager ? ['thread.view', 'thread.manage'] : ['thread.view'] },
    can: { ...thread.can, place: manager },
  };
}

beforeEach(() => { vi.clearAllMocks(); calls.start.mockResolvedValue({ id: 'retry-job' }); });
afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`offers native retry and an environment choice without the contradictory source notice (${locale})`, async () => {
    await setLocale(locale);
    const thread = blocked();
    show(<><BlockedBanner thread={thread} onProceed={() => undefined} /><SourcePreparation thread={thread} /></>);
    expect(screen.getByText(`${i18n.t('startRecovery.filesFailed', { environment: 'Main' })} ${i18n.t('startRecovery.held')}`)).toBeVisible();
    expect(screen.queryByText(i18n.t('source.prepare.pending'))).toBeNull();
    expect(screen.queryByText(/file job's environment changed/)).toBeNull();
    expect(calls.start).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('startRecovery.retry') }));
    await waitFor(() => expect(calls.start).toHaveBeenCalledWith('environment'));
    await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('startRecovery.elsewhere') })).toBeEnabled());
    await userEvent.click(screen.getByRole('button', { name: i18n.t('startRecovery.elsewhere') }));
    expect(calls.change).toHaveBeenCalledWith({ kind: 'thread', id: 'thread', name: 'Login' }, { kind: 'switch', focus: 'environment' });
  });

  it(`keeps recovery authority and displays a retry refusal (${locale})`, async () => {
    await setLocale(locale);
    const view = show(<BlockedBanner thread={blocked(false)} onProceed={() => undefined} />);
    expect(screen.queryByRole('button', { name: i18n.t('startRecovery.retry') })).toBeNull();
    expect(screen.queryByRole('button', { name: i18n.t('startRecovery.elsewhere') })).toBeNull();
    view.unmount();
    calls.start.mockRejectedValue(new Error('The machine is offline'));
    show(<BlockedBanner thread={blocked()} onProceed={() => undefined} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('startRecovery.retry') }));
    expect(await screen.findByText(i18n.t('errors.page'))).toBeVisible();
    expect(screen.getAllByRole('alert')).toHaveLength(2);
    expect(screen.getByRole('button', { name: i18n.t('startRecovery.elsewhere') })).toBeEnabled();
    expect(calls.change).not.toHaveBeenCalled();
  });

  it(`does not claim a kept message when no input is queued (${locale})`, async () => {
    await setLocale(locale);
    show(<BlockedBanner thread={{ ...blocked(), pending_inputs: 0 }} onProceed={() => undefined} />);
    expect(screen.getByText(i18n.t('startRecovery.filesFailed', { environment: 'Main' }))).toBeVisible();
    expect(screen.queryByText(i18n.t('startRecovery.held'))).toBeNull();
  });

  it(`uses reported placement independently of thread management (${locale})`, async () => {
    await setLocale(locale);
    const thread = blocked(false);
    const view = show(<BlockedBanner thread={{ ...thread, access: { ...thread.access, permissions: ['thread.view', 'thread.manage'] } }} onProceed={() => undefined} />);
    expect(screen.queryByRole('button', { name: i18n.t('startRecovery.elsewhere') })).toBeNull();
    expect(screen.queryByRole('button', { name: i18n.t('startRecovery.retry') })).toBeNull();
    view.rerender(<BlockedBanner thread={{ ...thread, can: { ...thread.can, place: true } }} onProceed={() => undefined} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('startRecovery.elsewhere') }));
    expect(calls.change).toHaveBeenCalledExactlyOnceWith({ kind: 'thread', id: 'thread', name: 'Login' }, { kind: 'switch', focus: 'environment' });
    expect(calls.start).not.toHaveBeenCalled();
  });

  it(`keeps an escape when the failed environment has disappeared (${locale})`, async () => {
    await setLocale(locale);
    show(<BlockedBanner thread={{ ...blocked(), environment: null }} onProceed={() => undefined} />);
    expect(screen.queryByRole('button', { name: i18n.t('startRecovery.retry') })).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('startRecovery.elsewhere') }));
    expect(calls.change).toHaveBeenCalledWith({ kind: 'thread', id: 'thread', name: 'Login' }, { kind: 'switch', focus: 'environment' });
    expect(calls.start).not.toHaveBeenCalled();
  });
}
