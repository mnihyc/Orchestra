/** Hints keep their ordinary and warning states; card reads stay independent of the number of names on screen. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';

import { queryClient } from '@/api/query';
import { ResourceHint, ResourceName } from '@/features/resource-facts/ResourceFacts';
import ResourceCard from '@/features/resource-facts/ResourceCard';
import type { ResourceSubject } from '@/features/resource-facts/facts';
import { detailEnvironment, detailHost } from './resource-detail-fixtures';
import { planProvider } from '../unit/provider-plan-fixtures';
import { choiceCap } from '../unit/resource-facts-fixtures';
import type { CapStatus } from '@/generated/api-types';
import { show } from './support';

const requests = vi.hoisted(() => ({ provider: vi.fn<(id: string) => void>(), caps: [] as CapStatus[] }));
vi.mock('@/api/client', async (load) => ({ ...await load<object>(), api: { GET: async (_path: string, options: { params: { path: { id: string } } }) => {
  const id = options.params.path.id;
  requests.provider(id);
  const { planProvider } = await import('../unit/provider-plan-fixtures');
  return { data: planProvider({ id, name: `Provider ${id}`, choice_usage: { caps: requests.caps, performance: [], observed_at: '2026-10-25T12:00:00Z' } }), error: undefined, response: new Response(null, { status: 200 }) };
} } }));
beforeEach(() => { requests.provider.mockClear(); requests.caps = []; });

const provider = planProvider();
const model = { model_name: 'model', display_name: 'Model', parameters: { context_window: 200_000 } };
it('reads missing model limits once and updates the same card hint from the permitted provider', async () => {
  requests.caps = [choiceCap({ state: 'reached' })];
  show(<ResourceCard subject={{ kind: 'model', model, providerId: 'model-provider' }} />);
  expect(await screen.findByRole('list', { name: 'Limits' })).toBeVisible();
  const hint = document.querySelector('[data-resource-hint="model"]');
  expect(hint).toHaveAttribute('data-warning', 'true');
  expect(hint).toHaveTextContent(/^At limit/);
  expect(requests.provider).toHaveBeenCalledExactlyOnceWith('model-provider');
});
const subjects: { subject: ResourceSubject; warning: boolean; text: RegExp }[] = [
  { subject: { kind: 'provider', provider }, warning: false, text: /81%/ },
  { subject: { kind: 'provider', provider: { ...provider, status: 'offline' } }, warning: true, text: /Unavailable/ },
  { subject: { kind: 'model', model, providerId: provider.id }, warning: false, text: /200K/ },
  { subject: { kind: 'model', model, providerId: provider.id, usage: { caps: [choiceCap({ state: 'reached' })], performance: [], observed_at: '2026-10-25T12:00:00Z' } }, warning: true, text: /At limit/ },
  { subject: { kind: 'host', host: detailHost }, warning: false, text: /running/ },
  { subject: { kind: 'host', host: { ...detailHost, full: true } }, warning: true, text: /^Full$/ },
  { subject: { kind: 'environment', environment: detailEnvironment }, warning: false, text: /Running/ },
  { subject: { kind: 'environment', environment: { ...detailEnvironment, status: 'offline' } }, warning: true, text: /Host offline/ },
];
for (const { subject, warning, text } of subjects) it(`renders a ${warning ? 'warning' : 'quiet'} ${subject.kind} hint from its permitted row`, () => {
  show(<ResourceHint subject={subject} />);
  const hint = screen.getByText(text).closest('[data-resource-hint]');
  expect(hint).not.toBeNull();
  expect(hint).toHaveAttribute('data-resource-hint', subject.kind);
  expect(hint).toHaveClass(warning ? 'text-warning-text' : 'text-text-muted');
  expect(requests.provider).not.toHaveBeenCalled();
});

for (const count of [1, 20]) it(`renders ${count} provider names with zero detail reads and reuses one opened detail`, async () => {
  const user = userEvent.setup();
  const cache = new QueryClient({ defaultOptions: queryClient.getDefaultOptions() });
  show(<QueryClientProvider client={cache}><div>{Array.from({ length: count }, (_, index) => <ResourceName key={index} subject={{ kind: 'provider', provider: { id: String(index), name: `Provider ${index}`, status: 'ready' } }} />)}</div></QueryClientProvider>);
  expect(requests.provider).not.toHaveBeenCalled();
  const first = screen.getByRole('button', { name: 'Provider 0' });
  await user.click(first);
  expect(await screen.findByRole('link', { name: 'Open provider' })).toHaveAttribute('href', '/resources/providers/0');
  expect(requests.provider).toHaveBeenCalledExactlyOnceWith('0');
  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('link', { name: 'Open provider' })).not.toBeInTheDocument());
  await user.click(first);
  expect(await screen.findByRole('link', { name: 'Open provider' })).toBeVisible();
  expect(requests.provider).toHaveBeenCalledExactlyOnceWith('0');
});

it('uses supplied list facts without another read and preserves the two windows and resets', async () => {
  const user = userEvent.setup();
  show(<ResourceName subject={{ kind: 'provider', provider }} />);
  await user.click(screen.getByRole('button', { name: provider.name }));
  expect(await screen.findByRole('meter', { name: '5-hour window' })).toBeVisible();
  expect(screen.getByRole('meter', { name: 'Weekly window (Fable)' })).toBeVisible();
  expect(screen.getByText('Reset time not set')).toBeVisible();
  expect(requests.provider).not.toHaveBeenCalled();
});

it('keeps private vendor facts and cap observations out of a resource card', () => {
  const privateProvider = planProvider({ choice_usage: { caps: [choiceCap({ state: 'reached' })], performance: [], observed_at: '2026-10-25T12:00:00Z' } });
  privateProvider.access.permissions = ['provider.use'];
  show(<ResourceCard subject={{ kind: 'provider', provider: privateProvider }} />);
  expect(screen.queryByRole('meter')).not.toBeInTheDocument();
  expect(screen.queryByRole('list', { name: 'Limits' })).not.toBeInTheDocument();
  expect(screen.queryByText('Active turns')).not.toBeInTheDocument();
  expect(requests.provider).not.toHaveBeenCalled();
});
