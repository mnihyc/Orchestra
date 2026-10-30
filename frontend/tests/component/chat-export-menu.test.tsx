/** A chat's served export decision needs no place read; every menu action keeps its real target. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ChatMenu } from '@/features/export/ChatMenu';
import type { Node } from '@/generated/api-types';
import type { ExportTarget } from '@/features/export/target';
import { chat } from '../doubles/chat-rows';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<() => Promise<Node>>(), export: vi.fn<(target: ExportTarget) => void>(), request: vi.fn<(target: ExportTarget) => void>() }));
vi.mock('@/api/nodes', () => ({ nodeQuery: (id: string) => ({ queryKey: ['menu-node', id], queryFn: calls.read }) }));
vi.mock('@/features/export/ExportHost', () => ({ openExport: calls.export, requestExportAccess: calls.request }));
beforeEach(() => { vi.clearAllMocks(); });

describe('the native chat export menu', () => {
  it('opens the allowed export from the chat answer, without fetching its place or guessing authority', async () => {
    const row = chat({ can: { send: false, moderate: false, export: true, reason: 'no_permission', admin_only: [], participating: true } });
    show(<ChatMenu chat={row} me="viewer" />);
    expect(calls.read).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: 'Actions for this chat' }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Export…' }));
    expect(calls.export).toHaveBeenCalledOnce();
    expect(calls.export.mock.calls[0]?.[0]).toMatchObject({ kind: 'chat', id: row.id, page: { id: row.node?.id, kind: 'topic' } });
    expect(calls.read).not.toHaveBeenCalled();
  });

  it('offers the existing node-page access request without an eager metadata read', async () => {
    const row = chat({ can: { send: true, moderate: false, export: false, reason: null, admin_only: [], participating: true } });
    show(<ChatMenu chat={row} me="member" />);
    await userEvent.click(screen.getByRole('button', { name: 'Actions for this chat' }));
    expect(screen.getByRole('menuitem', { name: 'Export…' })).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(screen.getByRole('menuitem', { name: 'Request access' }));
    expect(calls.request.mock.calls[0]?.[0]).toMatchObject({ kind: 'chat', id: row.id, page: { id: row.node?.id, kind: 'topic' } });
    expect(calls.read).not.toHaveBeenCalled();
  });

  it('keeps an inspecting admin outside a direct chat from exporting or requesting its membership', async () => {
    const row = chat({ kind: 'direct', node: null, can: { send: false, moderate: false, export: false, reason: 'inspecting', admin_only: [], participating: false } });
    show(<ChatMenu chat={row} me="outsider" />);
    await userEvent.click(screen.getByRole('button', { name: 'Actions for this chat' }));
    const menu = screen.getByRole('menu');
    expect(within(menu).getByRole('menuitem', { name: 'Export…' })).toHaveAttribute('aria-disabled', 'true');
    expect(within(menu).queryByRole('menuitem', { name: 'Request access' })).not.toBeInTheDocument();
    expect(calls.read).not.toHaveBeenCalled(); expect(calls.export).not.toHaveBeenCalled(); expect(calls.request).not.toHaveBeenCalled();
  });
});
