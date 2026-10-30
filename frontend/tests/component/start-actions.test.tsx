/** UB-24: a loading/refusal reason is separate from the ordinary Cancel/Create action group. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { NewThreadDialog } from '@/features/start/BindingDialog';
import type { PlaceRef } from '@/features/navigation/places';
import { setLocale } from '@/i18n';

import { show } from './support';

const state = vi.hoisted(() => ({ failed: false, start: vi.fn() }));
vi.mock('@/features/start/options', async (original) => ({ ...await original<object>(), useStartOptions: () => ({
  options: null, resolved: null, loading: !state.failed, failed: state.failed ? new Error('Options unavailable') : null,
  refreshing: false, retry: vi.fn(),
}) }));
vi.mock('@/features/start/start', async (original) => ({ ...await original<object>(), useStart: () => ({
  pending: false, error: null, start: state.start,
}) }));
vi.mock('@/api/resources', async (original) => ({ ...await original<object>(),
  providersQuery: () => ({ queryKey: ['start-action-providers'], queryFn: () => Promise.resolve([]) }),
  machinesQuery: { queryKey: ['start-action-machines'], queryFn: () => Promise.resolve([]) },
}));
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), nodeQuery: () => ({
  queryKey: ['start-action-topic'], queryFn: () => Promise.resolve({ access: { permissions: [] } }),
}) }));

const parent: PlaceRef = { id: 'topic', kind: 'topic', name: 'login', parentId: null, archived: false, status: 'open', folder: false };
beforeEach(() => { state.failed = false; vi.clearAllMocks(); });
afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const phase of ['loading', 'failed'] as const) {
  it(`keeps both actions together and retains the ${phase} refusal (${locale})`, async () => {
    await setLocale(locale);
    state.failed = phase === 'failed';
    const closed = vi.fn();
    const words = locale === 'en'
      ? { group: 'Thread actions', cancel: 'Cancel', create: 'Create thread', loading: 'Looking up what can be chosen…', failed: 'What can be chosen could not be loaded.' }
      : { group: '会话操作', cancel: '取消', create: '创建会话', loading: '正在查询可选项…', failed: '无法加载可选项。' };
    show(<NewThreadDialog parent={parent} open onClose={closed} />);
    const actions = screen.getByRole('group', { name: words.group });
    const cancel = within(actions).getByRole('button', { name: words.cancel });
    const create = within(actions).getByRole('button', { name: words.create });
    expect(within(actions).getAllByRole('button')).toHaveLength(2);
    if (phase === 'loading') {
      expect(screen.getAllByText(words.loading)).toHaveLength(1);
      expect(screen.queryByText(locale === 'en' ? 'Loading…' : '加载中…')).toBeNull();
    } else {
      expect(screen.getByRole('alert')).toBeVisible();
      expect(screen.queryByText(words.failed)).toBeNull();
    }
    expect(actions).not.toHaveTextContent(words[phase]);
    expect(create).toBeDisabled();
    await userEvent.click(create);
    expect(state.start).not.toHaveBeenCalled();
    await userEvent.click(cancel);
    expect(closed).toHaveBeenCalledOnce();
  });
}
