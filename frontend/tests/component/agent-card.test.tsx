/** Agent names open their own identity and respect an unreadable source thread. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { AgentChip } from '@/ui/AgentChip';
import { show } from './support';

const { lookup } = vi.hoisted(() => ({ lookup: vi.fn<(id: string) => Promise<unknown>>() }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), nodeQuery: (id: string) => ({ queryKey: ['agent-card-test', id], queryFn: () => lookup(id) }) }));
afterEach(() => { vi.clearAllMocks(); });

function draw(ui: ReactNode) {
  const root = createRootRoute({ component: () => ui });
  const thread = createRoute({ getParentRoute: () => root, path: '/thread/$threadId', component: () => null });
  const environment = createRoute({ getParentRoute: () => root, path: '/resources/environments/$id', component: () => null });
  const router = createRouter({ routeTree: root.addChildren([thread, environment]), history: createMemoryHistory({ initialEntries: ['/'] }) });
  return show(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><RouterProvider router={router} /></QueryClientProvider>);
}

describe('an agent’s identity', () => {
  it('reads on opening and shows the owner, thread, environment and model', async () => {
    lookup.mockResolvedValue({ id: 'thread-card', kind: 'thread', name: 'Fix the build', owner: { id: 'owner', kind: 'service', username: 'builder', display_name: 'Build owner', avatar: { text: 'B', color: 1 } }, environment: { id: 'room', name: 'Build workspace' }, environment_permissions: ['environment.view'], model: 'model-one', model_display: 'Research model', effort: 'high', binding: null });
    draw(<AgentChip name="Build agent" thread={{ id: 'thread-card', name: 'Fix the build', redacted: false }} />);
    const trigger = await screen.findByRole('button', { name: 'Build agent' });
    expect(lookup).not.toHaveBeenCalled();
    await userEvent.click(trigger);
    const card = await screen.findByRole('dialog', { name: 'Build agent' });
    expect(await within(card).findByText('Build owner')).toBeVisible();
    expect(within(card).getByRole('link', { name: 'Fix the build' })).toHaveAttribute('href', '/thread/thread-card');
    expect(within(card).getByRole('link', { name: 'Build workspace' })).toHaveAttribute('href', '/resources/environments/room');
    expect(within(card).getByText('Research model')).toBeVisible();
    expect(within(card).getByText('High',{exact:true})).toBeVisible();
    expect(card).not.toHaveTextContent('model-one');expect(card).not.toHaveTextContent(' · ');
    expect(lookup).toHaveBeenCalledTimes(1);
  });
  it('does not request or reveal a redacted thread’s runtime', async () => {
    draw(<AgentChip name="Other agent" thread={{ id: 'private', name: null, redacted: true }} />);
    await screen.findByRole('button', { name: 'Other agent' });
    await userEvent.tab();
    await userEvent.keyboard('{Enter}');
    expect(await screen.findByText('The agent’s thread is private to you. Its runtime details are not shown.')).toBeVisible();
    expect(lookup).not.toHaveBeenCalled();
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
  });
  it('keeps a name inside another control as readable text with its full name on hover', async () => {
    draw(<button type="button">Choose <AgentChip name="A long agent name" thread={{ id: 'thread-card', name: 'Fix the build', redacted: false }} /></button>);
    expect(await screen.findByRole('button', { name: 'Choose A long agent name' })).toBeVisible();
    expect(screen.getAllByRole('button')).toHaveLength(1);
    expect(screen.getByText('A long agent name')).toHaveAttribute('title', 'A long agent name');
    expect(lookup).not.toHaveBeenCalled();
  });
  it('keeps the card open after a failed read and retries without losing the name', async () => {
    lookup.mockRejectedValueOnce(new Error('The link went away.')).mockResolvedValue({ id: 'retry-card', kind: 'thread', name: 'Retry thread', owner: { id: 'owner', kind: 'service', username: 'builder', display_name: 'Build owner', avatar: { text: 'B', color: 1 } }, environment: null, environment_permissions: [], model: 'model-two', model_display: 'Second model', effort: null, binding: null });
    draw(<AgentChip name="Retry agent" thread={{ id: 'retry-card', name: 'Retry thread', redacted: false }} />);
    await userEvent.click(await screen.findByRole('button', { name: 'Retry agent' }));
    const card = await screen.findByRole('dialog', { name: 'Retry agent' });
    await userEvent.click(await within(card).findByRole('button', { name: /Try again/ }));
    expect(await within(card).findByText('Second model')).toBeVisible();
    expect(card).not.toHaveTextContent('model-two');
    expect(within(card).getByText('Retry agent')).toBeVisible();
  });
});
