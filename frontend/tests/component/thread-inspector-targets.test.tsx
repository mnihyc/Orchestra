/** Conversation inspectors do not treat a file's permission supplier as their conversation target. */
import { createRef } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, screen, waitFor } from '@testing-library/react';
import type * as Threads from '@/api/threads';
import type * as Live from '@/realtime/live';
import type * as Router from '@tanstack/react-router';
import type { ThreadOutlinePage } from '@/generated/api-types';
import { OutlineView } from '@/features/inspector/Outline';
import { ParametersView } from '@/features/inspector/Parameters';
import { ViewInstanceContext } from '@/features/workbench/instance';
import { ThreadReadProvider } from '@/realtime/threadReadContext';
import { resultThreadRead } from '@/realtime/threadScope';
import { threadWriter } from '@/realtime/stores/thread';
import { i18n } from '@/i18n';
import { snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ outline: vi.fn<typeof Threads.outlineQuery>(), parameters: vi.fn<typeof Threads.parametersQuery>(), follows: vi.fn() }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), useParams: () => ({ threadId: 'unrelated-route-thread' }) }));
vi.mock('@/api/threads', async (load) => {
  const actual = await load<typeof Threads>();
  calls.outline.mockImplementation((read) => ({ ...actual.outlineQuery(read), queryFn: () => new Promise<ThreadOutlinePage>(() => undefined) }));
  calls.parameters.mockImplementation(actual.parametersQuery);
  return { ...actual, outlineQuery: calls.outline, parametersQuery: calls.parameters };
});
vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useThreadFeed: (target: unknown) => { calls.follows(target); } }));
afterEach(() => { cleanup(); threadWriter.clear(); calls.outline.mockClear(); calls.parameters.mockClear(); calls.follows.mockClear(); });

it('file/workspace focus shows the existing empty state and opens no conversation or parameter reader', () => {
  show(<ViewInstanceContext.Provider value={{ id: 'following-panel', root: createRef(), target: { kind: 'environment', id: 'workspace-a', workspace: { environmentId: 'workspace-a', threadId: 'permission-supplier' } }, headingOwner: 'file-view', content: false, focusedContent: false }}>
    <ThreadReadProvider read={null}><OutlineView tab={{ id: 'outline', view: 'outline' }} area="right" /><ParametersView tab={{ id: 'parameters', view: 'parameters' }} area="right" /></ThreadReadProvider>
  </ViewInstanceContext.Provider>);
  expect(screen.getAllByText(i18n.t('inspector.frame.noThread'))).toHaveLength(2);
  expect(calls.outline).not.toHaveBeenCalled(); expect(calls.parameters).not.toHaveBeenCalled();
  expect(calls.follows.mock.calls.every(([target]) => target === null)).toBe(true);
});

it('an actual thread can read its outline without an environment, using its own history owner', async () => {
  const id = 'actual-thread', read = resultThreadRead(id, 'actual-outline'), cut = snapshot();
  threadWriter.reset(read, { ...cut, thread: { ...cut.thread, id, environment: null } }, false);
  show(<ViewInstanceContext.Provider value={{ id: 'actual-outline', root: createRef(), target: { kind: 'thread', id }, headingOwner: 'actual-view', content: false, focusedContent: false }}>
    <ThreadReadProvider read={read}><OutlineView tab={{ id: 'actual-outline', view: 'outline' }} area="right" /></ThreadReadProvider>
  </ViewInstanceContext.Provider>);
  await waitFor(() => expect(calls.outline).toHaveBeenCalledWith(read));
  expect(screen.queryByText(i18n.t('inspector.frame.noThread'))).not.toBeInTheDocument();
  expect(calls.follows.mock.calls.every(([target]) => target === id)).toBe(true);
});
