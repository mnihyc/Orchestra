/** Approval keeps consent while showing whether the capability was actually applied. */
import { cleanup, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { DecidedCard } from '@/features/approvals/Card';
import type { Approval, UserRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA } from '../unit/general-access-fixtures';
import { ALLOW_DENY, harnessApproval } from '../doubles/thread-rows';
import { show } from './support';

vi.mock('@/ui/UserChip', () => ({ UserChip: ({ user }: { user: UserRef }) => <span>{user.display_name}</span> }));
const base = harnessApproval('capability-request', { threadId: 'thread', itemId: 'item', title: 'Virtual machines', requester: ADA, request: { request_kind: 'command', detail: { command: 'review', cwd: '/work', reason: null }, options: ALLOW_DENY } });
function approved(state: 'applying' | 'failed' | 'applied'): Approval {
  return { ...base, kind: 'capability', request: { capability: 'kvm', label: 'Virtual machines', risk: 'high', reason: 'Run a VM', environment: { id: 'environment', name: 'Work', address: '', owner: ADA, placement: 'managed', status: 'running', ports_exposed: false }, host: { id: 'host', name: 'Host', address: '' }, consequences: [] }, state: 'approved', decided_by: ADA, decided_at: '2026-10-26T12:00:00Z', decision: { approve: true, application: { state, reason: state === 'failed' ? 'The live device helper exited with status 1; the environment restarted to restore its previous permissions' : null } } };
}
afterEach(async () => { cleanup(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('shows an approved but failed application and its cause in %s', async (locale) => {
  await setLocale(locale);
  show(<DecidedCard approval={approved('failed')} />);
  const status = screen.getByRole('status');
  expect(status).toHaveTextContent(i18n.t('approvals.capability.notGranted'));
  expect(status).toHaveTextContent('device helper exited with status 1');
  expect(status).toHaveTextContent('restarted to restore its previous permissions');
  expect(status).toHaveTextContent(ADA.display_name);
});

it('keeps an accepted grant visibly applying until the node confirms it', () => {
  show(<DecidedCard approval={approved('applying')} />);
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('approvals.capability.applying'));
});
