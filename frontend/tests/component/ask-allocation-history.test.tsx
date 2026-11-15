/** A creation's history readback must not replace the still-preparing local question. */
import { QueryClientProvider, useQuery } from '@tanstack/react-query';
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as AskApi from '@/api/ask';
import { askHistoryQuery, askQuery } from '@/api/ask';
import { client } from '@/api/client';
import { queryClient } from '@/api/query';
import type * as AuthApi from '@/api/auth';
import type * as NodesApi from '@/api/nodes';
import type * as ThreadsApi from '@/api/threads';
import type * as TreeStore from '@/realtime/stores/tree';
import { AskView } from '@/features/ask/AskView';
import { AskComposer, type AskComposerProps } from '@/features/composer/AskComposer';
import { useComposerStore } from '@/features/composer/Draft';
import type { AskGetReply, AskModel } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ASK_ID, askConversation, askModels, askSnapshot } from '../unit/ask-fixtures';
import { ACME, me } from '../doubles/fixtures';
import { threadDraft } from '../doubles/thread-rows';
import { show } from './support';

const wire = vi.hoisted(() => {
  const state: { record: AskGetReply | null; firstRead: Promise<AskGetReply> | null } = { record: null, firstRead: null };
  return { ...state, create: vi.fn<typeof AskApi.createAsk>(), send: vi.fn<typeof AskApi.sendAsk>() };
});
vi.mock('@/api/ask', async (original) => {
  const actual = await original<typeof AskApi>();
  return { ...actual, createAsk: wire.create, sendAsk: wire.send,
    askModelsQuery: (root: string) => ({ queryKey: ['allocation-models', root], queryFn: () => Promise.resolve(askModels(root)) }),
    askHistoryQuery: (root: string) => ({ ...actual.askHistoryQuery(root), retry: false,
      queryFn: () => Promise.resolve({ items: wire.record === null ? [] : [wire.record.conversation], next_cursor: null }) }),
    askQuery: (id: string) => ({ ...actual.askQuery(id), retry: false,
      queryFn: () => wire.firstRead ?? (wire.record === null ? Promise.reject(new Error('No allocated conversation')) : Promise.resolve(wire.record)) }),
  };
});
vi.mock('@/features/ask/useConversation', () => ({ useConversation: (id: string | null) => ({
  ...useQuery({ ...askQuery(id ?? ''), enabled: id !== null }), notice: '', tools: {}, streamError: null, deleted: false, reconnect: () => undefined,
}) }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof NodesApi>(),
  orgsQuery: { queryKey: ['allocation-orgs'], queryFn: () => Promise.resolve([]) } }));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof AuthApi>(),
  meQuery: { queryKey: ['allocation-person'], queryFn: () => Promise.resolve(me(1, 'ada', 'Ada Lovelace')) } }));
vi.mock('@/api/threads', async (original) => ({ ...await original<typeof ThreadsApi>(),
  draftQuery: (id: string) => ({ queryKey: ['allocation-draft', id], queryFn: () => Promise.resolve(null) }),
  saveDraft: (_id: string, content: Parameters<typeof ThreadsApi.saveDraft>[1]) => Promise.resolve(threadDraft(content)),
}));
vi.mock('@/features/access/AdminMode', async (loadOriginal) => ({ ...await loadOriginal<object>(), useAdminMode: () => ({ data: { as_admin: false } }), AdminModeEnableAction: () => null }));
vi.mock('@/features/workbench/ViewWindow', () => ({ ViewWindow: ({ actions, children }: { actions: ReactNode; children: ReactNode }) => <section>{actions}{children}</section> }));
vi.mock('@/features/workbench/views', async (loadOriginal) => ({ ...await loadOriginal<object>(), useViewShown: () => true }));
vi.mock('@/shell/screen', async (loadOriginal) => ({ ...await loadOriginal<object>(), useCurrentHeading: () => ({ nodeId: ACME.fixLogin, title: 'Fix login' }) }));
vi.mock('@/realtime/stores/tree', async (original) => ({ ...await original<typeof TreeStore>(), useNodePath: () => ({ orgId: ACME.org }) }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn(), openHelpReading: vi.fn() }));
vi.mock('@/features/people/NamedChip', () => ({ KnownNodeChip: () => <span>Organization</span>, TypedChip: () => <span>Organization</span> }));
vi.mock('@/features/composer/Composer', () => ({ Composer: ({ ask }: { ask: AskComposerProps }) => <AskComposer {...ask} /> }));
vi.mock('@/features/composer/Attachments', () => ({ AttachButton: () => null, AttachmentChips: () => null, UploadChips: () => null }));
vi.mock('@/features/ask/ModelChoice', () => ({ AskModelChoice: ({ disabled, onChange }: { disabled: boolean; onChange: (model: AskModel | null) => void }) =>
  <button disabled={disabled} onClick={() => onChange(askModels().default_model)}>Select API model</button> }));

const session = client.session.getState();
let epoch = session.readerVersion ?? 0;
beforeEach(() => {
  queryClient.clear(); vi.clearAllMocks(); useComposerStore.setState({ text: {}, parts: {} });
  client.session.setState({ ...session, readerVersion: ++epoch }); wire.record = null; wire.firstRead = null;
  wire.create.mockImplementation((body) => {
    wire.record = askSnapshot({ conversation: askConversation({ model: body.model ?? null, seq: 0 }), messages: [] });
    // The normal creation act retakes history before the initiating ensure finishes its snapshot read.
    queryClient.setQueryData(askHistoryQuery(ACME.org).queryKey, { pages: [{ items: [wire.record.conversation], next_cursor: null }], pageParams: [null] });
    return Promise.resolve({ conversation_id: ASK_ID, turn_id: null, seq: 0 });
  });
  wire.send.mockResolvedValue({ conversation_id: ASK_ID, turn_id: null, seq: 1 });
});
afterEach(async () => { cleanup(); queryClient.clear(); client.session.setState(session); useComposerStore.setState({ text: {}, parts: {} }); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const explicit of [false, true]) {
  it(`keeps the first ${explicit ? 'chosen-model' : 'default-model'} question sendable while allocation appears in history (${locale})`, async () => {
    await setLocale(locale);
    let deliver: (reply: AskGetReply) => void = () => undefined;
    wire.firstRead = new Promise<AskGetReply>((resolve) => { deliver = resolve; });
    show(<QueryClientProvider client={queryClient}><AskView area="right" tab={{ id: 'allocation', view: 'ask' }} /></QueryClientProvider>);
    const box = await screen.findByRole('textbox', { name: i18n.t('ask.composer') });
    await waitFor(() => expect(box).toBeEnabled());
    if (explicit) fireEvent.click(screen.getByRole('button', { name: 'Select API model' }));
    fireEvent.change(box, { target: { value: 'Keep the question during preparation' } });
    await waitFor(() => expect(wire.create).toHaveBeenCalledOnce());
    try {
      expect(screen.getByRole('textbox', { name: i18n.t('ask.composer') })).toBe(box);
      expect(box).toBeEnabled(); expect(box).toHaveValue('Keep the question during preparation');
      fireEvent.keyDown(box, { key: 'Enter' });
      expect(wire.send).not.toHaveBeenCalled();
    } finally {
      // Delivered whatever the checks above did, so that the conversation ends; its absence fails just below.
      const reply = wire.record;
      if (reply !== null) await act(() => { wire.firstRead = null; deliver(reply); return Promise.resolve(); });
    }
    expect(wire.record, 'The creation must have its native snapshot').not.toBeNull();
    await waitFor(() => expect(wire.send).toHaveBeenCalledOnce());
    expect(wire.send.mock.calls[0]?.[0]).toBe(ASK_ID);
    expect(wire.send.mock.calls[0]?.[1].content).toEqual([{ type: 'text', text: 'Keep the question during preparation' }]);
    expect(wire.send.mock.calls[0]?.[1].model).toEqual(explicit ? askModels().default_model : null);
    await waitFor(() => expect(box).toHaveValue(''));
  });
}

// Browser path: Topic → Ask → choose a model → type → history sees the allocation before its snapshot → Enter sends the retained question.
// 浏览器路径：话题 → 问一问 → 选择模型 → 输入 → 历史先出现新对话而快照仍在读取 → 回车发送保留的问题。
// The bilingual browser deciding path is ask-model-reset.spec.ts; mobile window paint is decided by ask-row-retry-replay.spec.ts.
