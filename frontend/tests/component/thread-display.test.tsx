/** Every thread icon slot reads the server presentation, including when execution state differs. */
import { act, cleanup, screen, within } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { NodeTree } from '@/features/navigation/NodeTree';
import { NamedChip } from '@/features/people/NamedChip';
import { StateFact } from '@/features/thread/Header';
import { ThreadCard } from '@/features/topic/ThreadCard';
import { KindIcon } from '@/features/everything/EverythingTable';
import type { Thread, ThreadDisplayState, TreeNode } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { ThreadStateIcon, LiveThreadStateIcon } from '@/ui/ThreadState';
import { ViewTab } from '@/ui/ViewTab';
import { display } from '../doubles/thread-display';
import { acmeTree, ACME } from '../doubles/fixtures';
import { treeWriter } from '@/realtime/stores/tree';
import { show } from './support';

const supplied = vi.hoisted(() => ({ thread: null as Thread | null, read: vi.fn() }));
vi.mock('@/api/references', () => ({ referencePreviewQuery: ({ id }: { id: string }) => ({ queryKey: ['state-reference', id], queryFn: () => {
  supplied.read();
  return Promise.resolve({ kind: 'node', id, node_kind: 'thread', title: supplied.thread?.name ?? 'Work', description: '',
    place: null, updated_at: null, updated_by: null, part: null, thread_display: supplied.thread?.display });
} }) }));
vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: ReactNode }) => <a href="#thread">{children}</a>, useRouter: () => ({ invalidate: vi.fn() }), useNavigate: () => vi.fn() }));
vi.mock('@/features/navigation/NodeMenu', () => ({ NodeContextMenu: ({ children }: { children: ReactNode }) => children, NodeMakeButton: () => null, NodeMenuButton: () => null, useIsPending: () => false }));
vi.mock('@/features/navigation/DocMenu', () => ({ DocContextMenu: ({ children }: { children: ReactNode }) => children, DocMenuButton: () => null }));
vi.mock('@/features/navigation/NodeDialogs', () => ({ openNodeDialog: vi.fn() }));
vi.mock('@/features/chat/open', () => ({ isPlainClick: () => true, useOpenPath: () => vi.fn() }));

const appearance = useAppearance.getState();
afterEach(() => { cleanup(); treeWriter.clear(); supplied.read.mockClear(); useAppearance.setState(appearance); });

const STATES = ['waiting_approval', 'blocked', 'failed', 'running', 'queued', 'held', 'finished', 'idle', 'archived'] as const;
const SHAPES = ['shield-question-mark', 'circle-slash', 'octagon-alert', 'loader-circle', 'clock-3', 'circle-pause', 'circle-check', 'message-square-text', 'archive'] as const;

function Tab({ state }: { state: ThreadDisplayState }) {
  return <ViewTab id="state-tab" panelId="state-panel" title="Work" icon={<ThreadStateIcon display={display(state)} />} active tabbable menu={[]} onSelect={vi.fn()} onClose={vi.fn()} />;
}

for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  it.each(STATES)(`keeps %s consistent in tree, tab, header, chip and row at ${width}px in ${theme}`, async (state) => {
    useAppearance.setState({ theme, resolvedTheme: theme });
    const icon = display(state, { queued_inputs: state === 'queued' ? 2 : 0, held_inputs: state === 'held' ? 2 : 0 });
    const fixture = acmeTree().find((row) => row.id === ACME.mainThread)!;
    const node: TreeNode = { ...fixture, archived: state === 'archived', thread: { ...fixture.thread!, state: 'idle', display: icon } };
    const root = acmeTree().find((row) => row.id === node.parent_id)!;
    const tree = { nodes: new Map([[root.id, root], [node.id, node]]), loaded: true, lastSeq: 1, error: null };
    const thread = { ...node, display: icon, current_turn_id: null, lock: null, blocked: null } as unknown as Thread;
    supplied.thread = thread;
    const view = show(<div data-theme={theme} style={{ width }}>
      <section data-testid="tree"><NodeTree tree={tree} rootId={root.id} view={{ showArchived: true }} expanded={new Set()} onExpandedChange={vi.fn()} currentId={null} locale="en" me="reader" /></section>
      <section data-testid="tab"><Tab state={state} /></section>
      <header data-testid="header"><StateFact thread={thread} /></header>
      <section data-testid="chip"><NamedChip path={`/thread/${node.id}`} kind="thread" id={node.id} /></section>
      <section data-testid="row"><KindIcon row={{ id: node.id, kind: 'thread', path: [], thread_display: icon }} /></section>
      <section data-testid="card"><ThreadCard thread={node} /></section>
    </div>);
    await within(screen.getByTestId('chip')).findByRole('img');
    expect(supplied.read).toHaveBeenCalledTimes(1);
    const shape = SHAPES[STATES.indexOf(state)];
    if (shape === undefined) throw new Error('Every state needs its own shape');
    for (const slot of ['tree', 'tab', 'header', 'chip', 'row', 'card']) {
      const element = screen.getByTestId(slot);
      const glyph = element.querySelector(`[data-work-state="${state}"] svg, .lucide-${shape}`);
      expect(glyph, slot).not.toBeNull();
      expect(element.querySelector('svg.lucide-'+shape), slot).not.toBeNull();
      if (slot !== 'header' && slot !== 'card') expect(within(element).getByRole('img')).toHaveAttribute('aria-label');
    }
    expect(view.container.querySelectorAll('[class~="motion-safe:animate-spin"]').length).toBe(state === 'running' ? 6 : 0);
  });
}

it('leaves an inaccessible reference as the plain glyph with no work state or private particulars', () => {
  const view = show(<ThreadStateIcon display={undefined} />);
  expect(view.container.querySelector('[data-work-state]')).toBeNull();
  expect(view.container.querySelector('svg.lucide-message-square-text')).not.toBeNull();
  expect(screen.queryByRole('img')).not.toBeInTheDocument();
});

it('keeps a retained tab current from its existing tree facts without opening a reader', () => {
  const row = acmeTree().find((node) => node.id === ACME.mainThread)!;
  const previous = display('finished', { seq: 10, read_seq: 0 });
  treeWriter.reset(ACME.org, [{ ...row, thread: { ...row.thread!, display: previous } }], 1);
  show(<ViewTab id="retained" panelId="panel" title="Work" active={false} tabbable menu={[]} onSelect={vi.fn()} onClose={vi.fn()}
    icon={<LiveThreadStateIcon display={previous} threadId={row.id} />} />);
  expect(screen.getByRole('img', { name: 'Finished' })).toBeVisible();
  act(() => treeWriter.display(row.id, display('idle', { seq: 10, read_seq: 10 })));
  expect(screen.getByRole('img', { name: 'Idle' })).toBeVisible();
  expect(supplied.read).not.toHaveBeenCalled();
});

it("keeps an archived place's own glyph and colour beside its archive mark", () => {
  const root = acmeTree().find((row) => row.id === ACME.org)!;
  const base = acmeTree().find((row) => row.id === ACME.web)!;
  const project: TreeNode = { ...base, name: 'Old project', parent_id: root.id, archived: true, color: 3 };
  const tree = { nodes: new Map([[root.id, root], [project.id, project]]), loaded: true, lastSeq: 1, error: null };
  show(<NodeTree tree={tree} rootId={root.id} view={{ showArchived: true }} expanded={new Set()} onExpandedChange={vi.fn()} currentId={null} locale="en" me="reader" />);
  const row = screen.getByRole('treeitem', { name: /Old project/ });
  expect(row.querySelector('[data-node-icon="project"]')).toHaveAttribute('data-node-color', '3');
  expect(within(row).getByRole('img', { name: 'Archived' })).toBeVisible();
});

it('names held-input recovery without claiming an approval request exists', () => {
  show(<ThreadStateIcon display={display('waiting_approval', { waiting_inputs: 2 })} />);
  expect(screen.getByRole('img', { name: '2 messages need attention' })).toBeVisible();
});
