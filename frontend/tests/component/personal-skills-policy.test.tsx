/** Native policy metadata explains personal-skill omission without opening a private inventory. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';
import { ApiError } from '@/api/client';
import type * as ShareApi from '@/api/share';
import type { PersonalSkillsPolicyView, ThreadKnowledgeView } from '@/api/share';
import { KnowledgeView } from '@/features/inspector/Knowledge';
import type { SettingSource, Thread, ThreadSharedTreeGetReply } from '@/generated/api-types';
import { setLocale } from '@/i18n';
import { snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<() => Promise<ThreadKnowledgeView>>() }));
const thread: Thread = { ...snapshot().thread, binding: null };
const source: SettingSource = { scope: 'project', node: { id: 'public-project', kind: 'project', name: 'Public project', redacted: false } };
const tree: ThreadSharedTreeGetReply = { path: '/', truncated: false, entries: [{
  id: 'shared-guide', kind: 'file', path: '/shared/guide.md', name: 'Shared guide', description: null,
  unit: null, parts: null, size: null, updated_at: null, children: null, readable: true, writable: false,
  manageable: false, deleted: false, via: 'share', shared_by: null, redacted: false,
}] };
const body = (policy?: PersonalSkillsPolicyView): ThreadKnowledgeView => ({
  read_first_paths: [{ id: 'shared-guide', path: '/shared/guide.md' }], shares: [],
  ...(policy === undefined ? {} : { personal_skills: policy }),
});
vi.mock('@/api/share', async (load) => {
  const actual = await load<typeof ShareApi>();
  return { ...actual,
    threadSharedQuery: (id: string) => ({ ...actual.threadSharedQuery(id), queryFn: calls.read }),
    sharedTreeQuery: (id: string, path = '/') => ({ ...actual.sharedTreeQuery(id, path), queryFn: () => Promise.resolve(tree) }),
  };
});
vi.mock('@/features/inspector/ThreadPanel', () => ({ ThreadPanel: ({ children }: { children: (thread: Thread) => ReactNode }) => children(thread) }));
vi.mock('@/features/people/AgentKnowledgeNotice', () => ({ AgentKnowledgeNotice: () => null }));

const words = {
  en: { checking: 'Checking personal skills', off: 'Personal skills off here', allowed: 'Personal skills allowed',
    source: 'Inherited from Public project', hidden: 'Set above', failed: 'Personal skills could not be checked',
    retry: 'Try again', unknown: 'Personal skills not reported', unsupported: 'Personal skills not supported',
    denied: 'Personal skills access denied', private: 'Personal skills unavailable in this view', sourceMissing: 'Source not reported' },
  'zh-CN': { checking: '正在检查个人技能', off: '此处已关闭个人技能', allowed: '允许个人技能',
    source: '继承自Public project', hidden: '由上级设置', failed: '无法检查个人技能',
    retry: '重试', unknown: '未报告个人技能策略', unsupported: '不支持个人技能',
    denied: '无权查看个人技能策略', private: '此视图无法查看个人技能策略', sourceMissing: '未报告来源' },
} as const;
const caches: QueryClient[] = [];
function draw() {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  caches.push(cache);
  show(<QueryClientProvider client={cache}><KnowledgeView area="right" tab={{ id: 'knowledge', view: 'knowledge' }} /></QueryClientProvider>);
  return cache;
}
beforeEach(async () => { vi.clearAllMocks(); await setLocale('en'); calls.read.mockResolvedValue(body({ state: 'off', reason: 'policy_off', sources: [source] })); });
afterEach(() => { cleanup(); caches.splice(0).forEach((cache) => cache.clear()); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows the native Off reason and exact readable setting source in ${locale}`, async () => {
    await setLocale(locale); draw();
    expect(await screen.findByText(words[locale].off)).toBeVisible();
    expect(screen.getByRole('link', { name: words[locale].source })).toHaveAttribute('href', '/project/public-project/settings?section=policies&setting=agents.personal_skills');
    expect(await screen.findByText('Shared guide')).toBeVisible();
    expect(calls.read).toHaveBeenCalledOnce();
  });

  it(`keeps private source identities and counts out of the line in ${locale}`, async () => {
    await setLocale(locale);
    calls.read.mockResolvedValue(body({ state: 'off', reason: 'policy_off', sources: [
      { scope: 'project', node: { id: 'private-source-id', kind: 'project', name: 'PRIVATE SOURCE', redacted: true } },
      { scope: 'org', node: null },
    ] }));
    draw(); await screen.findByText(words[locale].off);
    const panel = screen.getByTestId('thread-knowledge');
    expect(within(panel).getAllByText(words[locale].hidden, { exact: false })).toHaveLength(1);
    expect(panel.innerHTML).not.toContain('PRIVATE SOURCE');
    expect(panel.innerHTML).not.toContain('private-source-id');
    expect(within(panel).queryByRole('link')).not.toBeInTheDocument();
    expect(screen.getByText('Shared guide')).toBeVisible();
  });

  it(`retries only the existing knowledge supplier after unavailable policy metadata in ${locale}`, async () => {
    await setLocale(locale);
    calls.read.mockResolvedValueOnce(body({ state: 'unavailable' })).mockResolvedValue(body({ state: 'allowed' }));
    draw(); expect(await screen.findByText(words[locale].failed)).toBeVisible();
    expect(screen.getByText('Shared guide')).toBeVisible();
    await userEvent.setup().click(screen.getByRole('button', { name: words[locale].retry }));
    expect(await screen.findByText(words[locale].allowed)).toBeVisible();
    expect(calls.read).toHaveBeenCalledTimes(2);
    expect(screen.queryByText(words[locale].off)).not.toBeInTheDocument();
  });

  it.each(['missing', 'unsupported'] as const)(`keeps %s distinct from policy Off in ${locale}`, async (state) => {
    await setLocale(locale);
    calls.read.mockResolvedValue(body(state === 'missing' ? undefined : { state }));
    draw(); expect(await screen.findByText(state === 'missing' ? words[locale].unknown : words[locale].unsupported)).toBeVisible();
    expect(screen.queryByText(words[locale].off)).not.toBeInTheDocument();
    expect(screen.queryByText(words[locale].allowed)).not.toBeInTheDocument();
    expect(screen.getByText('Shared guide')).toBeVisible();
  });

  it(`keeps a known Off state honest when its source is not reported in ${locale}`, async () => {
    await setLocale(locale);
    calls.read.mockResolvedValue(body({ state: 'off', reason: 'policy_off', sources: [] }));
    draw(); expect(await screen.findByText(words[locale].off)).toBeVisible();
    expect(screen.getByText(words[locale].sourceMissing)).toBeVisible();
    expect(screen.getByRole('button', { name: words[locale].retry })).toBeVisible();
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
    expect(screen.getByText('Shared guide')).toBeVisible();
  });

  it(`shows checking without inventing a value or hiding the shared tree in ${locale}`, async () => {
    await setLocale(locale); calls.read.mockImplementation(() => new Promise(() => undefined)); draw();
    expect(screen.getByText(words[locale].checking)).toBeVisible();
    expect(await screen.findByText('Shared guide')).toBeVisible();
    expect(screen.queryByText(words[locale].off)).not.toBeInTheDocument();
  });

  it.each([403, 404])(`keeps a refused/private %s read generic in ${locale}`, async (status) => {
    await setLocale(locale);
    calls.read.mockRejectedValue(new ApiError({ type: 'orch:error:read', code: status === 403 ? 'authz.denied' : 'not_found', status, title: '', detail: '', request_id: '' }, null));
    draw(); expect(await screen.findByText(status === 403 ? words[locale].denied : words[locale].private)).toBeVisible();
    expect(screen.getByText('Shared guide')).toBeVisible();
    expect(screen.queryByText(words[locale].off)).not.toBeInTheDocument();
  });

  it(`repairs the visible policy after an inherited settings act without reading unopened threads in ${locale}`, async () => {
    await setLocale(locale);
    calls.read.mockResolvedValueOnce(body({ state: 'off', reason: 'policy_off', sources: [source] })).mockResolvedValue(body({ state: 'allowed' }));
    const cache = draw(); await screen.findByText(words[locale].off);
    cache.setQueryData(['thread-shared', 'unopened'], body({ state: 'off', reason: 'policy_off', sources: [] }));
    const accepted = findAct('PUT', '/nodes/public-project/settings/agents.personal_skills', { answer: { key: 'agents.personal_skills', value: true } });
    expect(accepted).not.toBeNull();
    await act(async () => { await applyChanges(accepted?.changes ?? [], cache); });
    await waitFor(() => expect(screen.getByText(words[locale].allowed)).toBeVisible());
    expect(calls.read).toHaveBeenCalledTimes(2);
  });
}

// EN: Thread → Knowledge → Personal skills off here → inherited setting source; Retry refreshes this same read and never opens personal skills.
// 中文：线程 → 知识 → 此处已关闭个人技能 → 上级设置来源；重试只刷新同一读取，不打开个人技能清单。
