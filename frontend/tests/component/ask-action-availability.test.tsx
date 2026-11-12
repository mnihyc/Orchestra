/** Ask header actions explain their own native state without borrowing message authority. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as AskApi from '@/api/ask';
import { adminModeKey } from '@/api/adminMode';
import { queryClient } from '@/api/query';
import type * as NodesApi from '@/api/nodes';
import type * as AuthApi from '@/api/auth';
import type * as TreeStore from '@/realtime/stores/tree';
import { AskView } from '@/features/ask/AskView';
import type { AskGetReply } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { askConversation, askModels, askSnapshot } from '../unit/ask-fixtures';
import { show } from './support';

const world = vi.hoisted(() => {
  const state: { snapshot: AskGetReply | undefined; error: unknown } = { snapshot: undefined, error: null };
  return { ...state, loading: false, deleted: false, export: vi.fn<typeof AskApi.exportAsk>(), save: vi.fn(), read: vi.fn(), keep: vi.fn() };
});
vi.mock('@/api/ask', async (original) => ({ ...await original<typeof AskApi>(), exportAsk: world.export, askReader: () => () => undefined,
  askHistoryQuery: () => ({ queryKey: ['action-history'], queryFn: () => Promise.resolve({ items: [], next_cursor: null }), initialPageParam: null, getNextPageParam: () => undefined }),
  askModelsQuery: () => ({ queryKey: ['action-models'], queryFn: () => Promise.resolve(askModels()) }) }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof NodesApi>(), orgsQuery: { queryKey: ['action-orgs'], queryFn: () => Promise.resolve([]) } }));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof AuthApi>(), meQuery: { queryKey: ['action-me'], queryFn: () => Promise.resolve(null) } }));
vi.mock('@/features/export/save', () => ({ saveFile: world.save }));
vi.mock('@/features/ask/useConversation', () => ({ useConversation: () => ({ data: world.snapshot, isLoading: world.loading, error: world.error,
  deleted: world.deleted, notice: '', tools: {}, streamError: null, refetch: world.read, reconnect: world.read }) }));
vi.mock('@/features/ask/Keep', () => ({ KeepAsk: () => { world.keep(); return <div role="dialog" aria-label="Keep destination" />; } }));
vi.mock('@/features/access/AdminMode', () => ({ useAdminMode: () => ({ data: { as_admin: true } }), AdminModeEnableAction: () => null }));
vi.mock('@/features/workbench/ViewWindow', () => ({ ViewWindow: ({ actions, children }: { actions: ReactNode; children: ReactNode }) => <section>{actions}{children}</section> }));
vi.mock('@/features/workbench/views', () => ({ useViewShown: () => true }));
vi.mock('@/shell/screen', () => ({ useCurrentHeading: () => ({ nodeId: 'topic', title: 'Topic' }) }));
vi.mock('@/realtime/stores/tree', async (original) => ({ ...await original<typeof TreeStore>(), useNodePath: () => ({ orgId: 'root' }) }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn(), openHelpReading: vi.fn() }));
vi.mock('@/features/composer/Composer', () => ({ Composer: () => null }));
vi.mock('@/features/ask/ModelAvailability', () => ({ AskModelAvailability: ({ children }: { children: (available: boolean) => ReactNode }) => children(true) }));

const appearance = useAppearance.getState();
beforeEach(() => {
  vi.clearAllMocks(); world.snapshot = askSnapshot({ conversation: askConversation({ payer_id: null }), messages: [] });
  world.loading = false; world.error = null; world.deleted = false; world.export.mockResolvedValue(new Blob(['{}']));
  queryClient.clear(); queryClient.setQueryData(adminModeKey, { available: true, as_admin: true });
});
afterEach(async () => { cleanup(); queryClient.clear(); useAppearance.setState(appearance); await setLocale('en'); });
const panel = (inspecting = false, existing = true) => <AskView area="right" tab={{ id: 'ask-actions', view: 'ask', of: { ...(existing ? { conversation: 'conversation' } : {}), ...(inspecting ? { inspect: '1' } : {}) } }} />;
const exporting = () => screen.getByRole('button', { name: i18n.t('ask.export') });
const keeping = () => screen.getByRole('button', { name: i18n.t('ask.keep') });
async function hasReason(button: HTMLElement, reason: string) {
  expect(button).toHaveAttribute('aria-disabled', 'true');
  await userEvent.tab(); button.focus();
  await waitFor(() => expect(screen.getByRole('tooltip')).toHaveTextContent(reason));
  await userEvent.keyboard('{Escape}'); button.blur();
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`keeps Export available in inspection and while running, with a separate Keep reason in ${locale} / ${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme });
    const base = askConversation({ payer_id: null });
    world.snapshot = askSnapshot({ messages: [], conversation: { ...base, inspecting: true, can: { ...base.can, message: false, keep: false, export: true } } });
    const view = show(panel(true));
    await hasReason(keeping(), i18n.t('ask.inspection'));
    await userEvent.click(keeping()); expect(world.keep).not.toHaveBeenCalled();
    expect(exporting()).not.toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(exporting()); await waitFor(() => expect(world.save).toHaveBeenCalledOnce());
    world.snapshot = askSnapshot({ messages: [], conversation: { ...base, state: 'running', can: { ...base.can, message: false, keep: false } } });
    view.rerender(panel());
    await hasReason(keeping(), i18n.t('ask.actions.answerRunning'));
    await userEvent.click(keeping()); expect(world.keep).not.toHaveBeenCalled();
    expect(exporting()).not.toHaveAttribute('aria-disabled', 'true');
  });

  it(`distinguishes no conversation, checking, failed read and expiry in ${locale} / ${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme }); world.snapshot = undefined;
    const view = show(panel(false, false));
    await hasReason(exporting(), i18n.t('ask.actions.noConversation'));
    await hasReason(keeping(), i18n.t('ask.actions.noConversation'));
    world.loading = true; view.rerender(panel());
    await hasReason(exporting(), i18n.t('ask.actions.checking'));
    world.loading = false; world.error = new Error('Private source or hidden audience must not be named'); view.rerender(panel());
    await hasReason(keeping(), i18n.t('ask.actions.unavailable'));
    expect(screen.queryByText('Private source or hidden audience must not be named')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') })); expect(world.read).toHaveBeenCalledOnce();
    world.error = null; world.deleted = true; view.rerender(panel());
    await hasReason(exporting(), i18n.t('ask.deleted'));
    expect(world.export).not.toHaveBeenCalled(); expect(world.keep).not.toHaveBeenCalled();
  });

  it(`uses each served action flag independently of message authority in ${locale} / ${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme }); const base = askConversation({ payer_id: null });
    world.snapshot = askSnapshot({ messages: [], conversation: { ...base, can: { ...base.can, message: true, export: false, keep: false } } });
    const view = show(panel());
    await hasReason(exporting(), i18n.t('ask.actions.exportDenied'));
    await hasReason(keeping(), i18n.t('ask.actions.keepDenied'));
    await userEvent.click(exporting()); await userEvent.click(keeping()); expect(world.export).not.toHaveBeenCalled(); expect(world.keep).not.toHaveBeenCalled();
    world.snapshot = askSnapshot({ messages: [], conversation: { ...base, can: { ...base.can, message: false, export: true, keep: true } } });
    view.rerender(panel());
    expect(keeping()).not.toHaveAttribute('aria-disabled', 'true'); await userEvent.click(keeping()); expect(world.keep).toHaveBeenCalled();
  });

  it(`names closed and already-kept conversations without refusing permitted Export in ${locale} / ${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme }); const base = askConversation({ payer_id: null });
    for (const state of ['closed', 'kept'] as const) {
      world.snapshot = askSnapshot({ messages: [], conversation: { ...base, state, can: { ...base.can, message: false, keep: false, export: true } } });
      const view = show(panel());
      await hasReason(keeping(), i18n.t(state === 'closed' ? 'ask.closed' : 'ask.kept'));
      expect(exporting()).not.toHaveAttribute('aria-disabled', 'true');
      await userEvent.click(keeping()); expect(world.keep).not.toHaveBeenCalled(); view.unmount();
    }
  });

  it(`keeps a pending Export focusable with its state and ignores a second press in ${locale} / ${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme });
    let finish: (blob: Blob) => void = () => undefined;
    world.export.mockReturnValueOnce(new Promise<Blob>((resolve) => { finish = resolve; }));
    show(panel());
    await userEvent.click(exporting());
    try {
      expect(exporting()).toHaveAttribute('aria-busy', 'true');
      await hasReason(exporting(), i18n.t('ask.actions.exporting'));
      await userEvent.click(exporting()); expect(world.export).toHaveBeenCalledOnce();
      expect(keeping()).not.toHaveAttribute('aria-disabled', 'true');
    } finally { await act(() => { finish(new Blob(['{}'])); return Promise.resolve(); }); }
    await waitFor(() => expect(world.save).toHaveBeenCalledOnce());
  });
}

// Browser path: Topic → Ask → focus Export/Keep → empty/checking/refused reason → inspection Export still downloads.
// 浏览器路径：话题 → 问一问 → 聚焦导出或保留 → 查看未创建、读取中或拒绝的原因 → 管理员只读查看仍可导出。
// The bilingual width/theme walk is in implicit-actions.spec.ts.
