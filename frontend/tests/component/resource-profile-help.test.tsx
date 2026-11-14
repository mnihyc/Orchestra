/** Version and raw-profile controls retain their shared explanation for managers and ordinary readers. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Resources from '@/api/resources';
import type * as Auth from '@/api/auth';
import type * as Trail from '@/features/resources/trail';
import type { Agent } from '@/generated/api-types';
import { AgentScreen } from '@/features/resources/agents/Profile';
import { i18n, setLocale } from '@/i18n';
import { detailAccess, detailAgent } from './resource-detail-fixtures';
import { me } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ agent: vi.fn<() => Promise<Agent>>(), writes: vi.fn() }));
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof Resources>(), updateAgent: calls.writes,
  agentQuery: (id: string) => ({ queryKey: ['profile-help', id], queryFn: calls.agent }),
  harnessKindsQuery: { queryKey: ['profile-help-kinds'], queryFn: () => Promise.resolve([]) },
}));
vi.mock('@/api/auth', async (load) => ({ ...await load<typeof Auth>(),
  meQuery: { queryKey: ['profile-help-me'], queryFn: () => Promise.resolve(me(1, 'ada', 'Ada')) },
}));
vi.mock('@/features/resources/trail', async (load) => ({ ...await load<typeof Trail>(), useResourceHeading: () => undefined }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));
beforeEach(() => { vi.clearAllMocks(); });
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it.each([false, true])(`explains the actual profile settings without writing in ${locale}, manage=%s`, async (manage) => {
  await setLocale(locale);
  const user = userEvent.setup();
  calls.agent.mockResolvedValue({ ...detailAgent, access: manage ? detailAccess : { ...detailAccess, permissions: ['agent.view'], administers: false } });
  const view = show(<AgentScreen agentId={detailAgent.id} tab="profile" onTab={vi.fn()} />);
  await screen.findByRole('heading', { name: detailAgent.name });
  await waitFor(() => expect(view.container.querySelector('[data-help-control="agent-pin"]')).not.toBeNull());
  if (manage) {
    const details = view.container.querySelector('details[data-resource-details] > summary');
    if (!(details instanceof HTMLElement)) throw new Error('The raw profile has its technical disclosure.');
    await user.click(details);
  }
  await waitFor(() => expect(view.container.querySelector('[data-help-control="agent-profile"]')).not.toBeNull());
  const term = i18n.t('help.hints.agent-pin.title');
  const question = screen.getByRole('button', { name: i18n.t('help.hint.label', { term }) });
  expect(question).toBeVisible();
  await user.click(question);
  expect(await screen.findByRole('dialog', { name: term })).toBeVisible();
  expect(calls.writes).not.toHaveBeenCalled();
});
