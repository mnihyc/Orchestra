/** A historical vendor refusal stays visible without inventing an Orch quota-bypass requirement. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import type * as ThreadsApi from '@/api/threads';
import { BlockedBanner } from '@/features/thread/BlockedBanner';
import { waitingWords } from '@/features/timeline/waiting';
import type { ThreadLimits } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { failedStart } from './failed-start-fixtures';
import { choiceCap } from '../unit/resource-facts-fixtures';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

const server = vi.hoisted(() => {
  const limits: ThreadLimits = { reached: [], bypass_all: false };
  return { limits };
});
vi.mock('@/api/threads', async (load) => ({ ...await load<typeof ThreadsApi>(),
  limitsQuery: (id: string) => ({ queryKey: ['vendor-banner-limits', id], initialData: server.limits, queryFn: () => Promise.resolve(server.limits) }),
}));

it('keeps the vendor error and removes the old bypass and ask-manager suggestions', () => {
  const provider = planProvider();
  server.limits = { bypass_all: true, reached: [{ cap: null, provider_limit: { provider, as_of: '2026-10-25T12:00:00Z', resets_at: null, window: 'weekly' }, can_bypass: true, passed_by: null, ask: [], ask_more: 0 }] };
  const native = failedStart().thread;
  const thread = { ...native, state: 'blocked' as const, lock: null, binding: null,
    blocked: { reason: 'provider_exhausted' as const, detail: 'The vendor refused the previous request.', sandbox: null },
    access: { ...native.access, permissions: ['thread.view', 'thread.interact'] } };
  const proceed = vi.fn();
  show(<BlockedBanner thread={thread} onProceed={proceed} />);
  const words=waitingWords(thread, i18n.t);
  if(words===null)throw new Error('The provider refusal fixture must have waiting text');
  expect(screen.getByRole('alert')).toHaveTextContent(words.trim());
  expect(screen.queryByRole('button', { name: i18n.t('thread.blocked.proceed') })).not.toBeInTheDocument();
  expect(screen.queryByText(i18n.t('thread.blocked.manager', { who: thread.owner.display_name }))).not.toBeInTheDocument();
  expect(proceed).not.toHaveBeenCalled();
});

it('still offers explicit bypass for an actual configured cap', async () => {
  server.limits = { bypass_all: true, reached: [{ cap: choiceCap({ state: 'reached' }), provider_limit: null, can_bypass: true, passed_by: null, ask: [], ask_more: 0 }] };
  const native = failedStart().thread;
  const thread = { ...native, state: 'blocked' as const, lock: null, binding: null,
    blocked: { reason: 'quota_exceeded' as const, detail: null, sandbox: null },
    access: { ...native.access, permissions: ['thread.view', 'thread.interact'] } };
  const proceed = vi.fn();
  show(<BlockedBanner thread={thread} onProceed={proceed} />);
  await userEvent.setup().click(screen.getByRole('button', { name: i18n.t('thread.blocked.proceed') }));
  expect(proceed).toHaveBeenCalledTimes(1);
});
