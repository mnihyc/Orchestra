/** A mounted Ports panel keeps the served door's unknown, failed and refused states distinct. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type { DoorState } from '@/features/files/door';
import { PortsView } from '@/features/ports/View';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { snapshot } from '../unit/continuation-fixtures';
import { portsEnvironment } from './ports-fixtures';
import { show } from './support';

let door: DoorState = { kind: 'loading' };
const read = vi.hoisted(() => vi.fn());
const retry = vi.hoisted(() => vi.fn());
vi.mock('@/features/files/door', () => ({ useRouteThread: () => 'thread-1', useThreadDoor: () => door }));
vi.mock('@/realtime/live', async (loadOriginal) => ({ ...await loadOriginal<object>(), refreshThread: retry }));
vi.mock('@/api/environments', async (original) => ({ ...await original<object>(), environmentQuery: (id: string) => ({ queryKey: ['implicit-port-environment', id], queryFn: read }) }));
vi.mock('@/features/browser/View', async (loadOriginal) => ({ ...await loadOriginal<object>(), portsBrowser: () => undefined, showBrowserBeside: vi.fn() }));
vi.mock('@/features/ports/Ports', () => ({ PortsPanel: () => <div data-testid="current-ports">Current port inventory</div> }));

function ready(allowed: boolean): DoorState {
  const thread = snapshot().thread;
  if (thread.environment === null) throw new Error('the fixture needs its actual environment');
  return { kind: 'ready', access: { door: { environmentId: thread.environment.id, threadId: thread.id },
    thread: { ...thread, environment_permissions: allowed ? ['ports.view'] : ['thread.view'] },
    environment: thread.environment, directory: '', mayView: false, mayManage: false, mayWatchProcesses: false, mayShell: false } };
}
afterEach(async () => { cleanup(); read.mockReset(); retry.mockReset(); door = { kind: 'loading' }; await setLocale('en'); });

for (const language of ['en', 'zh-CN'] as const) {
  it(`keeps checking and lost Ports View visible without reading or retaining inventory in ${language}`, async () => {
    await setLocale(language); const t = i18n.getFixedT(language);
    read.mockResolvedValue(portsEnvironment);
    const view = show(<PortsView />);
    expect(screen.getByRole('status')).toHaveTextContent(t('ports.view.checking'));
    expect(read).not.toHaveBeenCalled();
    door = ready(true); view.rerender(<PortsView />);
    expect(await screen.findByTestId('current-ports')).toBeVisible();
    await waitFor(() => expect(read).toHaveBeenCalledOnce());
    door = ready(false); view.rerender(<PortsView />);
    expect(screen.getByRole('status')).toHaveTextContent(t('ports.view.denied'));
    expect(screen.queryByTestId('current-ports')).not.toBeInTheDocument();
    expect(read).toHaveBeenCalledOnce();
    door = { kind: 'none', why: 'no_environment' }; view.rerender(<PortsView />);
    expect(screen.getByText(t('ports.view.noEnvironment'))).toBeVisible();
    expect(screen.queryByText(t('ports.view.denied'))).not.toBeInTheDocument();
  });
  it(`keeps a failed door separate from permission refusal and retries the original read in ${language}`, async () => {
    await setLocale(language); const t = i18n.getFixedT(language);
    door = { kind: 'error', error: new NetworkError('offline') };
    show(<PortsView />);
    expect(screen.getByRole('alert')).toBeVisible();
    expect(screen.queryByText(t('ports.view.denied'))).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: t('common.tryAgain') }));
    expect(retry).toHaveBeenCalledWith('thread-1');
    expect(read).not.toHaveBeenCalled();
  });
}
