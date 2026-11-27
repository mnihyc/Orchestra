/** Host ports save at once; only a change that closes open ports says so first, in one line. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as PortApi from '@/api/ports';
import { HostPortsDeclaration } from '@/features/ports/HostDeclaration';
import type { HostPortsReview } from '@/features/ports/presentation';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({ review: null as unknown as HostPortsReview, save: vi.fn() }));
vi.mock('@/api/ports', async (original) => ({ ...await original<typeof PortApi>(), saveHostPorts: calls.save,
  hostPortsQuery: (id: string) => ({ queryKey: ['host-ports-at-once', id], queryFn: () => calls.review, retry: false }) }));

const proposal = { bind: 'all', address: 'host.example', ranges: [{ from: 41000, to: 41015 }] };
function review(current: HostPortsReview['current'], allocations: HostPortsReview['allocations']): HostPortsReview {
  return { current, proposal, allocations, revision: 4, acknowledged: null, can_edit: true, reason: null };
}
afterEach(async () => { cleanup(); vi.resetAllMocks(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`saves a first declaration at once, with no review step (${locale})`, async () => {
    await setLocale(locale);
    calls.review = review(null, []);
    calls.save.mockResolvedValue({});
    const user = userEvent.setup();
    show(<HostPortsDeclaration hostId="host" />);
    await user.click(await screen.findByRole('button', { name: i18n.t('ports.hostForm.edit') }));
    expect(screen.queryByRole('button', { name: /review/i })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: i18n.t('ports.hostForm.save') }));
    await waitFor(() => expect(calls.save).toHaveBeenCalledExactlyOnceWith('host', proposal, 4));
  });
}

it('saves a changed declaration whose openings the host did not list, and says once that another Listen on closes them', async () => {
  calls.review = review(proposal, null);
  calls.save.mockResolvedValue({});
  const user = userEvent.setup();
  show(<HostPortsDeclaration hostId="host" />);
  await user.click(await screen.findByRole('button', { name: i18n.t('ports.hostForm.edit') }));
  expect(screen.queryByText(i18n.t('ports.hostForm.closed'))).not.toBeInTheDocument();
  await user.click(screen.getByRole('radio', { name: i18n.t('choiceCatalog.hostBinding.one.name') }));
  await user.type(screen.getByRole('textbox', { name: i18n.t('ports.hostForm.one') }), '10.0.0.2');
  expect(screen.getByText(i18n.t('ports.hostForm.closed'))).toBeVisible();
  const save = screen.getByRole('button', { name: i18n.t('ports.hostForm.save') });
  expect(save).toBeEnabled();
  await user.click(save);
  await waitFor(() => expect(calls.save).toHaveBeenCalledExactlyOnceWith('host', { ...proposal, bind: '10.0.0.2' }, 4));
});

it('clearing a declaration with open ports names them in one line first; with none it clears at once', async () => {
  const environment = { id: 'env', kind: 'environment' as const, name: 'Web preview', redacted: false };
  calls.review = review(proposal, [{ opening_id: 'o1', environment, port: 41001, transports: 'tcp', state: 'open' } as unknown as NonNullable<HostPortsReview['allocations']>[number]]);
  calls.save.mockResolvedValue({});
  const user = userEvent.setup();
  const view = show(<HostPortsDeclaration hostId="host" />);
  await user.click(await screen.findByRole('button', { name: i18n.t('ports.hostForm.edit') }));
  await user.click(screen.getByRole('button', { name: i18n.t('ports.hostForm.clear') }));
  expect(screen.getByText(i18n.t('ports.hostForm.closes', { count: 1 }))).toBeVisible();
  expect(calls.save).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('actions.show.label') }));
  expect(screen.getByText('Web preview')).toBeVisible();
  await user.click(screen.getByRole('button', { name: i18n.t('ports.hostForm.clear') }));
  await waitFor(() => expect(calls.save).toHaveBeenCalledExactlyOnceWith('host', null, 4));
  view.unmount();
  calls.save.mockClear();
  calls.review = review(proposal, []);
  show(<HostPortsDeclaration hostId="host-empty" />);
  await user.click(await screen.findByRole('button', { name: i18n.t('ports.hostForm.edit') }));
  await user.click(screen.getByRole('button', { name: i18n.t('ports.hostForm.clear') }));
  await waitFor(() => expect(calls.save).toHaveBeenCalledExactlyOnceWith('host-empty', null, 4));
});
