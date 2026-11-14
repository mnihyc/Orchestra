/** A visible row keeps its failed authority read distinct from the native export decision. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as DocumentsApi from '@/api/documents';
import type * as NodesApi from '@/api/nodes';
import { ApiError } from '@/api/client';
import { DocMenuButton } from '@/features/navigation/DocMenu';
import { NodeMenuButton } from '@/features/navigation/NodeMenu';
import type { Node } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { snapshot } from '../unit/continuation-fixtures';
import { commentDocument } from './comment-document-fixture';
import { show } from './support';

const calls = vi.hoisted(() => ({ document: vi.fn<() => Promise<DocumentsApi.Document>>(), node: vi.fn<() => Promise<Node>>(),
  export: vi.fn(), request: vi.fn() }));
vi.mock('@/api/documents', async (original) => ({ ...await original<typeof DocumentsApi>(),
  documentQuery: (id: string) => ({ queryKey: ['export-read-document', id], queryFn: calls.document, retry: false }) }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof NodesApi>(),
  nodeQuery: (id: string) => ({ queryKey: ['export-read-node', id], queryFn: calls.node, retry: false }) }));
vi.mock('@/features/documents/Readme', () => ({ useReadmeEntry: () => null }));
vi.mock('@/features/export/ExportHost', async (loadOriginal) => ({ ...await loadOriginal<object>(), openExport: calls.export, requestExportAccess: calls.request }));
beforeEach(() => { vi.clearAllMocks(); calls.document.mockReset(); calls.node.mockReset(); });
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const kind of ['document', 'thread'] as const) {
  it(`repeats only the failed ${kind} authority read, then uses the served refusal in ${locale}`, async () => {
    await setLocale(locale);
    const thread = snapshot().thread;
    const document = { ...commentDocument, access: { ...commentDocument.access, permissions: ['doc.view'] } };
    calls.document.mockRejectedValueOnce(new NetworkError('failed')).mockResolvedValue(document);
    calls.node.mockRejectedValueOnce(new NetworkError('failed')).mockResolvedValue({ ...thread, can: { ...thread.can, export: false } });
    show(kind === 'document'
      ? <DocMenuButton doc={{ id: document.id, title: document.title, node: document.node, archived: false, owner: document.owner }} />
      : <NodeMenuButton place={{ id: thread.id, kind: 'thread', name: thread.name, parentId: thread.parent_id, archived: false, status: null, folder: false }} />);
    await userEvent.click(screen.getByRole('button', { name: new RegExp(kind === 'document' ? document.title : thread.name) }));
    expect(await screen.findByText(i18n.t('network.failed'))).toBeVisible();
    expect(screen.getByRole('menuitem', { name: i18n.t('export.menu.export') })).toHaveAttribute('aria-disabled', 'true');
    expect(screen.queryByRole('menuitem', { name: i18n.t('export.menu.request') })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('menuitem', { name: i18n.t('common.tryAgain') }));
    await waitFor(() => expect(kind === 'document' ? calls.document : calls.node).toHaveBeenCalledTimes(2));
    await userEvent.click(screen.getByRole('button', { name: new RegExp(kind === 'document' ? document.title : thread.name) }));
    expect(await screen.findByRole('menuitem', { name: i18n.t('export.menu.request') })).toBeVisible();
    expect(screen.getByRole('menuitem', { name: i18n.t('export.menu.export') })).toHaveAttribute('aria-disabled', 'true');
    expect(calls.export).not.toHaveBeenCalled(); expect(calls.request).not.toHaveBeenCalled();
  });
}

it('does not turn a private read refusal into an owner disclosure or access request', async () => {
  calls.document.mockRejectedValue(new ApiError({ type: 'orch:error:not_found', status: 404, code: 'not_found', title: 'Not found', detail: 'Hidden source, owner, audience', request_id: 'private-read' }, null));
  show(<DocMenuButton doc={{ id: commentDocument.id, title: 'Already visible document', node: commentDocument.node, archived: false, owner: null }} />);
  await userEvent.click(screen.getByRole('button', { name: /Already visible document/ }));
  await waitFor(() => expect(calls.document).toHaveBeenCalledOnce());
  expect(await screen.findByRole('menuitem', { name: i18n.t('export.menu.export') })).toHaveAttribute('aria-disabled', 'true');
  await waitFor(() => expect(screen.getByRole('menu')).not.toHaveTextContent(i18n.t('export.menu.checking')));
  expect(screen.getByRole('menu')).not.toHaveTextContent('Hidden source, owner, audience');
  expect(screen.queryByRole('menuitem', { name: i18n.t('export.menu.request') })).not.toBeInTheDocument();
  expect(screen.queryByRole('menuitem', { name: i18n.t('common.tryAgain') })).not.toBeInTheDocument();
});

// Browser path: open a visible row's menu → failed read → Retry → current Export refusal, without an export request.
// 浏览器路径：打开可见行的菜单 → 读取失败 → 重试 → 显示当前导出拒绝原因，不发起导出请求。
// The bilingual width/theme walk is in implicit-actions.spec.ts.
