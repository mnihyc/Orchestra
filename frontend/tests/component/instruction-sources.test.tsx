/** Instruction sources distinguish native evidence, unknown cuts, separate index bounds and permitted file doors. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { InstructionSources } from '@/features/inspector/InstructionSources';
import type { InstructionSource, ThreadInstructions } from '@/generated/api-types';
import { show } from './support';

const calls = vi.hoisted(() => ({ data: null as ThreadInstructions | null, open: vi.fn() }));
vi.mock('@/api/instructions', () => ({ threadInstructionsQuery: (thread: string) => ({ queryKey: ['test-sources', thread], queryFn: () => Promise.resolve(calls.data) }) }));
vi.mock('@/features/files/FileTabs', () => ({ openFile: calls.open }));
const source: InstructionSource = { order: 0, kind: 'memory', label: 'MEMORY.md', evidence: 'documented_rule', scope: null, source_environment: { id: 'env-1', name: 'Main', address: '/environments/main', placement: 'connected', ports_exposed: false, status: 'running', owner: { id: 'user-1', username: 'ada', display_name: 'Ada', kind: 'human', avatar: { text: 'A', color: 0 } } }, path: '/workspace/web/.orch/memory/MEMORY.md', workspace_path: 'web/.orch/memory/MEMORY.md', bytes: 50_000, loaded_bytes: 25_600, cut_bytes: 24_400, lines: 500, loaded_lines: 200, line_limit: 200, byte_limit: 25_600, reason: null, readable: true, editable: true };
function draw() { show(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><InstructionSources threadId="thread-1" /></QueryClientProvider>); }
beforeEach(() => { calls.open.mockReset(); calls.data = { segment_id: 'segment-1', observed_at: null, context_revision: null, sources: [source], standing: { characters: 20, limit: 40_000, pending_next_turn: false } }; });

describe('the sources a harness reads', () => {
  it('shows the line and byte limits separately and opens an authorized source from the workspace root', async () => {
    const user = userEvent.setup(); draw();
    await screen.findByText(/line limit 200/);
    await user.click(screen.getByText('Instructions this agent reads'));
    expect(screen.getByText(/byte limit/)).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'MEMORY.md' }));
    expect(calls.open).toHaveBeenCalledWith({ environmentId: 'env-1', threadId: 'thread-1', path: 'web/.orch/memory/MEMORY.md' });
  });
  it('shows a native include and its unknown loaded size separately from documented limits', async () => {
    calls.data!.sources = [{ ...source, kind: 'file', label: 'rules.md', evidence: 'reported', loaded_bytes: null, loaded_lines: null, cut_bytes: null, line_limit: null, byte_limit: null, parent_path: '/workspace/CLAUDE.md', load_reason: 'include' }];
    const user = userEvent.setup(); draw();
    await screen.findByText('Imported from /workspace/CLAUDE.md');
    await user.click(screen.getByText('Instructions this agent reads'));
    expect(screen.getByText(/loaded.*Size unknown/)).toBeVisible();
  });
  it('gives a personal source its safe label and no open act or private path', async () => {
    calls.data!.sources = [{ ...source, label: 'Personal instructions', path: null, workspace_path: null, source_environment: null, parent_path: null, readable: false, editable: false, bytes: null, loaded_bytes: null, cut_bytes: null, lines: null }];
    const user = userEvent.setup(); draw();
    await screen.findByText('Personal instructions');
    await user.click(screen.getByText('Instructions this agent reads'));
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
    expect(screen.queryByText('/workspace/web/.orch/memory/MEMORY.md')).not.toBeInTheDocument();
    expect(calls.open).not.toHaveBeenCalled();
  });
});
