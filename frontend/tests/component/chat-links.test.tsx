/** A chat address can reveal a never-mounted view and never duplicates a previously targeted one. */
import { act, render, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useChatLinks } from '@/features/chat/links';

const world = vi.hoisted(() => ({ request: { panel: false, chat: '', message: '' }, node: undefined as string | undefined, current: undefined as string | undefined, reveal: vi.fn() }));
vi.mock('@tanstack/react-router', () => ({ useRouterState: () => world.request }));
vi.mock('@/shell/screen', () => ({ useCurrentHeading: () => ({ nodeId: world.node }), currentHeading: () => ({ nodeId: world.current }) }));
vi.mock('@/features/workbench/openView', () => ({ revealView: world.reveal }));
function Frame() { useChatLinks(); return null; }
afterEach(() => { world.request = { panel: false, chat: '', message: '' }; world.node = undefined; world.current = undefined; vi.clearAllMocks(); });

it('opens the addressed node after its metadata arrives, without mounting a hidden body to trigger the act', async () => {
  world.request = { panel: true, chat: '', message: '' };
  const view = render(<Frame />);
  expect(world.reveal).not.toHaveBeenCalled();
  world.node = 'thread'; world.current = 'thread';
  view.rerender(<Frame />);
  await waitFor(() => expect(world.reveal).toHaveBeenCalledExactlyOnceWith('chat', { node: 'thread' }));
  // Opening a different target from a shortcut does not replay the unchanged address request.
  view.rerender(<Frame />);
  expect(world.reveal).toHaveBeenCalledTimes(1);
});

it('ignores a previous page’s metadata that was cleared at the route boundary', async () => {
  world.request = { panel: true, chat: '', message: '' }; world.node = 'old';
  render(<Frame />);
  await act(async () => { await Promise.resolve(); });
  expect(world.reveal).not.toHaveBeenCalled();
});
