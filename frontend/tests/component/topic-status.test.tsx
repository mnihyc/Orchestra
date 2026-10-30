/** UA-11: one compact status menu preserves current revisions, pending intent, refusal and Undo. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import { changeTopicStatus } from '@/api/day100';
import { usePendingPlaces } from '@/features/navigation/NodeMenu';
import { TopicStatusChip } from '@/features/topic/Status';
import type { Topic } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { toast } from '@/ui/Toast';
import { TooltipProvider } from '@/ui/Tooltip';
import { org } from '../doubles/fixtures';
import { ADA } from '../unit/general-access-fixtures';

vi.mock('@/api/day100', () => ({ changeTopicStatus: vi.fn() }));
vi.mock('@/ui/Toast', () => ({ toast: vi.fn(() => vi.fn()) }));
vi.mock('@/features/navigation/NodeMenu', async () => {
  const { create } = await import('zustand');
  const pending = create<{ ids: ReadonlySet<string> }>(() => ({ ids: new Set() }));
  return {
    usePendingPlaces: pending,
    useIsPending: (id: string) => pending((state) => state.ids.has(id)),
  };
});

const base = org('release', 'release', 'Release', ADA);
const topic: Topic = {
  ...base, kind: 'topic', org_id: 'acme', parent_id: 'project',
  access: { ...base.access, permissions: ['topic.view', 'topic.interact'] },
  status: 'open', done_at: null, thread_display_state: null, environments: [], default_environment_id: null, thread_count: 0,
  attention: { running: 0, waiting_approval: 0, blocked: 0 },
  work_attention: { uncommitted: 0, unpushed: 0, failed: 0 }, last_activity_at: null, seq: 1, revision: 7,
};
const done: Topic = { ...topic, status: 'done', revision: 8 };
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const beforeAppearance = useAppearance.getState();
const previousTheme = document.documentElement.dataset.theme;
const wasDark = document.documentElement.classList.contains('dark');
const caches: QueryClient[] = [];

function draw(ui: ReactNode) {
  const cache = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  caches.push(cache);
  return render(ui, { wrapper: ({ children }: { children: ReactNode }) =>
    <QueryClientProvider client={cache}><TooltipProvider>{children}</TooltipProvider></QueryClientProvider> });
}

function statusName(status: 'open' | 'done') {
  return i18n.t('places.topic.changeStatus', { status: i18n.t(`places.topic.statuses.${status}`) });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(changeTopicStatus).mockReset().mockResolvedValue(done);
  vi.mocked(toast).mockReset().mockImplementation(() => vi.fn());
  usePendingPlaces.setState({ ids: new Set() });
});
afterEach(async () => {
  cleanup();
  for (const cache of caches.splice(0)) cache.clear();
  useAppearance.setState(beforeAppearance);
  if (previousTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = previousTheme;
  document.documentElement.classList.toggle('dark', wasDark);
  await setLocale('en');
});

describe('the topic status chip', () => {
  it.each(modes)('says the state once and opens a checked keyboard menu in %s/%s', async (locale, theme) => {
    await setLocale(locale);
    useAppearance.setState({ theme, resolvedTheme: theme });
    document.documentElement.dataset.theme = theme;
    document.documentElement.classList.toggle('dark', theme === 'dark');
    const user = userEvent.setup();
    const view = draw(<TopicStatusChip topic={topic} allowed />);
    const chip = screen.getByRole('button', { name: statusName('open') });
    expect(chip.textContent).toBe(i18n.t('places.topic.statuses.open'));
    expect(view.container.querySelectorAll('[data-status-chip]')).toHaveLength(1);
    expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
    await user.tab();
    expect(chip).toHaveFocus();
    await user.keyboard('{Enter}');
    const menu = screen.getByRole('menu', { name: i18n.t('places.topic.statusLabel') });
    expect(within(menu).getAllByRole('menuitemradio')).toHaveLength(2);
    const open = within(menu).getByRole('menuitemradio', { name: i18n.t('places.topic.statuses.open') });
    expect(open).toHaveAttribute('aria-checked', 'true');
    await user.click(open);
    expect(changeTopicStatus).not.toHaveBeenCalled();
    await user.click(chip);
    await user.keyboard('{Escape}');
    expect(chip).toHaveFocus();
    await user.keyboard('{Enter}');
    await screen.findByRole('menu', { name: i18n.t('places.topic.statusLabel') });
    await user.keyboard('{ArrowDown}{Enter}');
    await waitFor(() => expect(changeTopicStatus).toHaveBeenCalledExactlyOnceWith(topic.id, 'done', 7));
    view.rerender(<TopicStatusChip topic={done} allowed />);
    await waitFor(() => expect(screen.getByRole('button', { name: statusName('done') })).toBeEnabled());
    expect(screen.getByRole('button', { name: statusName('done') }).textContent).toBe(i18n.t('places.topic.statuses.done'));
  });

  it('shows the requested state in one busy chip and waits for the returned revision before Undo', async () => {
    let finish: ((next: Topic) => void) | undefined;
    vi.mocked(changeTopicStatus).mockImplementationOnce(() => new Promise<Topic>((resolve) => { finish = resolve; }));
    const user = userEvent.setup();
    const view = draw(<TopicStatusChip topic={topic} allowed />);
    await user.click(screen.getByRole('button', { name: statusName('open') }));
    await user.click(screen.getByRole('menuitemradio', { name: 'Done' }));
    const waiting = await screen.findByRole('button', { name: statusName('done') });
    expect(waiting).toBeDisabled();
    expect(waiting).toHaveAttribute('aria-busy', 'true');
    expect(waiting.querySelectorAll('[class~="motion-safe:animate-spin"]')).toHaveLength(1);
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    expect(changeTopicStatus).toHaveBeenCalledExactlyOnceWith(topic.id, 'done', 7);
    expect(toast).not.toHaveBeenCalled();
    await act(() => Promise.resolve(finish?.(done)));
    view.rerender(<TopicStatusChip topic={done} allowed />);
    await waitFor(() => expect(screen.getByRole('button', { name: statusName('done') })).toBeEnabled());
    const notice = vi.mocked(toast).mock.calls.find(([entry]) => entry.tone === 'success')?.[0];
    expect(notice?.action?.label).toBe('Undo');
    vi.mocked(changeTopicStatus).mockResolvedValue({ ...topic, revision: 9 });
    await act(() => Promise.resolve(notice?.action?.run()));
    await waitFor(() => expect(changeTopicStatus).toHaveBeenLastCalledWith(topic.id, 'open', 8));
    notice?.action?.run();
    expect(changeTopicStatus).toHaveBeenCalledTimes(2);
  });

  it('returns to the served state on refusal and keeps a way to try again', async () => {
    vi.mocked(changeTopicStatus).mockRejectedValueOnce(new ApiError({
      type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision', status: 409,
      title: 'Changed meanwhile', detail: 'Read the current topic and try again.', request_id: 'status-review',
    }, null));
    const user = userEvent.setup();
    draw(<TopicStatusChip topic={topic} allowed />);
    await user.click(screen.getByRole('button', { name: statusName('open') }));
    await user.click(screen.getByRole('menuitemradio', { name: 'Done' }));
    const chip = await screen.findByRole('button', { name: statusName('open') });
    await waitFor(() => expect(chip).toBeEnabled());
    expect(chip.textContent).toBe('Open');
    expect(vi.mocked(toast).mock.calls.some(([notice]) => notice.tone === 'danger' && notice.title === 'Release could not be changed')).toBe(true);
    await user.click(chip);
    expect(screen.getByRole('menuitemradio', { name: 'Open' })).toHaveAttribute('aria-checked', 'true');
  });

  it.each([false, true])('shows a plain state without a menu when archived=%s or the person cannot interact', (archived) => {
    const view = draw(<TopicStatusChip topic={{ ...topic, archived }} allowed={archived} />);
    expect(view.container.querySelector('[data-status-chip]')).toHaveTextContent('Open');
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
    expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
    expect(changeTopicStatus).not.toHaveBeenCalled();
  });

  it('blocks another status act while a different topic operation is pending', () => {
    usePendingPlaces.setState({ ids: new Set([topic.id]) });
    draw(<TopicStatusChip topic={topic} allowed />);
    expect(screen.getByRole('button', { name: statusName('open') })).toBeDisabled();
    expect(changeTopicStatus).not.toHaveBeenCalled();
  });

});
