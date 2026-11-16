/** Captured source facts share the existing disclosure and retire with their reader. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { FileFactBody, FileSourceDisclosure, imageDeliveryFragment } from '@/features/files/FileFacts';
import type { FileSourceFacts, NativeImageReceipt, PreviewAllowance, FilesPlan } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { PlanLists } from '@/features/change/Lists';
import { filesLines } from '@/features/change/plan';
import { show } from './support';

const at = '2026-11-15T05:00:00Z';
const facts: FileSourceFacts = { policy: { kind: 'workspace', writable: true, workspace_exclusions: true }, observed_at: at,
  inclusion: { governing_rule_sources: [{ kind: 'configured', path: null }, { kind: 'local', path: 'sub/.orch_ignore' }, { kind: 'local', path: null }], deeper_rules_override: true, parent_excluded: false, internal_rules_applied: true },
  watch: { native: true, recursive: false, fallback_interval_secs: 97, observation_id: 'one', source: 'machine_configuration' } };
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps native source facts on intent and uses its captured interval after a new observation (${locale})`, async () => {
    await setLocale(locale);
    const read = vi.fn().mockResolvedValue({ transfer: null });
    const view = show(<FileSourceDisclosure facts={facts} asOf={at} readTransfer={read} />);
    expect(screen.queryByText('sub/.orch_ignore')).not.toBeInTheDocument();
    expect(read).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
    const body = await screen.findByRole('dialog');
    expect(body).toHaveTextContent('sub/.orch_ignore');
    expect(body).toHaveTextContent(i18n.t('fileFacts.notShared'));
    expect(body).toHaveTextContent('37');
    expect(read).toHaveBeenCalledTimes(1);
    view.rerender(<FileSourceDisclosure facts={{ ...facts, watch: null }} asOf={at} readTransfer={read} />);
    expect(body).not.toHaveTextContent('37');
    expect(body).toHaveTextContent(i18n.t('factDisclosure.notReported'));
  });

  it(`keeps missing old policy distinct from read-only mounts and whole owned roots (${locale})`, async () => {
    await setLocale(locale);
    const view = show(<FileFactBody showSource />);
    expect(screen.getAllByText(i18n.t('factDisclosure.notReported')).length).toBeGreaterThan(0);
    view.rerender(<FileFactBody showSource facts={{ ...facts, policy: { kind: 'mount', writable: false, workspace_exclusions: false }, inclusion: null, watch: null }} />);
    expect(screen.getByText(i18n.t('fileFacts.readOnlyMount'))).toBeVisible();
    expect(screen.queryByText('sub/.orch_ignore')).not.toBeInTheDocument();
    view.rerender(<FileFactBody showSource facts={{ ...facts, policy: { kind: 'owned_data', writable: true, workspace_exclusions: false }, inclusion: null, watch: null }} />);
    expect(screen.getByText(i18n.t('fileFacts.ownedWhole'))).toBeVisible();
  });

  it(`names the refused preview accounting and the actual native image route independently (${locale})`, async () => {
    await setLocale(locale);
    const allowance: PreviewAllowance = { kind: 'pixels', limit: 99, actual_if_known: 100, source: { scope: 'system', node: null } };
    const receipt: NativeImageReceipt = { part: 0, forwarded_image: null, applied_delivery: 'path', allowance_exceeded: true, native_image_bytes: 14, source: null, resolved_at: at };
    const view = show(<FileFactBody allowance={allowance} imageDelivery={receipt} />);
    expect(screen.getByText(i18n.t('fileFacts.allowanceKind.pixels'))).toBeVisible();
    expect(screen.queryByText(i18n.t('fileFacts.allowanceKind.viewer_bytes'))).not.toBeInTheDocument();
    expect(imageDeliveryFragment(receipt, i18n.t)).toBe(i18n.t('fileFacts.imageByPath'));
    expect(screen.getByText(i18n.t('fileFacts.notShared'))).toBeVisible();
    view.rerender(<FileFactBody imageDelivery={{ ...receipt, native_image_bytes: 0, allowance_exceeded: false }} />);
    expect(screen.getByText(i18n.t('fileFacts.imagesUsePaths'))).toBeVisible();
    expect(screen.queryByText(i18n.t('fileFacts.imageByPath'))).not.toBeInTheDocument();
  });
}

it('keeps only the exact machine transfer receipt and does not derive its expiry from current settings', async () => {
  const read = vi.fn().mockResolvedValue({ transfer: { transfer_id: 'a'.repeat(64), state: 'interrupted', last_piece_at: at, expires_at: '2026-11-15T05:02:00Z', idle_retention_secs: 120, retention_source: { scope: 'default', node: null } } });
  const view = show(<FileSourceDisclosure facts={facts} readTransfer={read} />);
  await userEvent.click(screen.getByRole('button', { name: 'Details' }));
  await waitFor(() => expect(screen.getByRole('dialog')).toHaveTextContent('2 min'));
  expect(screen.getByRole('dialog')).toHaveTextContent(i18n.t('fileFacts.lastPiece'));
  view.rerender(<FileSourceDisclosure key="another-source" facts={null} />);
  expect(screen.queryByText(i18n.t('fileFacts.lastPiece'))).not.toBeInTheDocument();
  expect(read).toHaveBeenCalledTimes(1);
});

it('withdraws previously visible source names when the ordinary source read loses authority', async () => {
  const { ApiError } = await import('@/api/client');
  const read = vi.fn().mockResolvedValueOnce({ transfer: null }).mockRejectedValueOnce(new ApiError({ status: 403, code: 'authz.denied', title: '', detail: '', type: '', request_id: '' }, null));
  show(<FileSourceDisclosure facts={facts} readTransfer={read} />);
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Details' }));
  expect(await screen.findByText('sub/.orch_ignore')).toBeVisible();
  await waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  await user.keyboard('{Escape}');
  await user.click(screen.getByRole('button', { name: 'Details' }));
  await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
  await waitFor(() => expect(screen.queryByText('sub/.orch_ignore')).not.toBeInTheDocument());
  expect(screen.getByRole('dialog')).toHaveTextContent(i18n.t('fileFacts.notShared'));
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps an old copy policy unknown while a measured owned root keeps its captured policy (${locale})`, async () => {
    await setLocale(locale);
    const plan: FilesPlan = { source: 'cached', source_as_of: at, file_count: 1, total_bytes: 100, bytes_to_upload: 0, bytes_to_download: 100,
      missing: { count: 0, bytes: null, sample: [] }, path_map: { from: 'before', to: 'after' },
      home: { measured: true, file_count: 1, total_bytes: 50 }, file_states: null, private_views: [],
      source_policy: null, home_policy: { kind: 'owned_data', workspace_exclusions: false, writable: true } };
    const view = show(<PlanLists changes={filesLines(plan, locale, i18n.t)} stays={[]} ends={[]} />);
    expect(screen.getByText(i18n.t('fileFacts.ownedWhole'))).toBeVisible();
    await userEvent.click(screen.getAllByRole('button', { name: i18n.t('common.details') })[0]!);
    expect(screen.getByRole('dialog')).toHaveTextContent(i18n.t('factDisclosure.notReported'));
    expect(screen.getByRole('dialog')).not.toHaveTextContent(i18n.t('fileFacts.applied'));
    await userEvent.keyboard('{Escape}');
    await userEvent.click(screen.getAllByRole('button', { name: i18n.t('common.details') })[1]!);
    expect(screen.getByRole('dialog')).toHaveTextContent(i18n.t('fileFacts.wholeRoot'));
    expect(screen.getByRole('dialog')).toHaveTextContent(i18n.t('fileFacts.requiresPermission'));
    await userEvent.keyboard('{Escape}');
    view.rerender(<PlanLists changes={filesLines({ ...plan, home_policy: null }, locale, i18n.t)} stays={[]} ends={[]} />);
    expect(screen.queryByText(i18n.t('fileFacts.ownedWhole'))).not.toBeInTheDocument();
  });
}

for (const locale of ['en', 'zh-CN'] as const) it(`keeps an intentional machine path distinct from an owned snapshot root (${locale})`, async () => {
  await setLocale(locale);
  show(<FileFactBody showSource facts={{ ...facts, policy: { kind: 'machine', writable: true, workspace_exclusions: false }, inclusion: null, watch: null }} />);
  expect(screen.getByText(i18n.t('fileReach.machine'))).toBeVisible();
  expect(screen.queryByText(i18n.t('fileFacts.ownedWhole'))).not.toBeInTheDocument();
});
