/** A candidate never borrows its first endpoint's schema for an unknown or different source request. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Resources from '@/api/resources';
import { CandidateEditor } from '@/features/settings/CallsRules';
import type { ValueControlProps } from '@/features/settings/ValueControl';
import { i18n, setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';
const calls = vi.hoisted(() => ({ format: vi.fn<(protocol: string | null | undefined) => void>() }));
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof Resources>(),
  providerQuery: (id: string) => ({ queryKey: ['format-provider', id], queryFn: () => Promise.resolve(planProvider({ id, protocols: id === 'source' ? ['openai-responses'] : ['anthropic-messages', 'openai-responses'] })) }),
  providerModelsQuery: (id: string) => ({ queryKey: ['format-models', id], queryFn: () => Promise.resolve([]) }),
  agentsQuery: () => ({ queryKey: ['format-agents'], queryFn: () => Promise.resolve([]) }),
}));
vi.mock('@/features/settings/ProviderChoice', () => ({ ProviderDefaultChoice: ({ label }: ValueControlProps) => <input aria-label={label} readOnly /> }));
vi.mock('@/features/models/RequestOptions', () => ({ ModelRequestOptions: ({ protocol }: { protocol?: string | null }) => { calls.format(protocol); return <output>{protocol ?? 'raw'}</output>; } }));
afterEach(async () => { cleanup(); calls.format.mockClear(); await setLocale('en'); });
const step = { type: 'failover' as const, candidate: { provider_id: 'target', model: 'model', params: {} } };

it.each(['en', 'zh-CN'] as const)('passes the source format even when another target endpoint comes first in %s', async (locale) => {
  await setLocale(locale); show(<CandidateEditor purpose="summary" sourceProviderId="source" step={step} onChange={() => undefined} />);
  await waitFor(() => expect(calls.format).toHaveBeenLastCalledWith('openai-responses'));
  expect(screen.getByText('openai-responses', { selector: 'code' })).toBeVisible();
});
it.each(['en', 'zh-CN'] as const)('keeps generic future calls explicit and raw rather than guessing target0 in %s', async (locale) => {
  await setLocale(locale); show(<CandidateEditor purpose={undefined} step={step} onChange={() => undefined} />);
  await waitFor(() => expect(screen.getByRole('combobox', { name: i18n.t('providerFields.preview') })).toBeVisible());
  expect(calls.format).toHaveBeenLastCalledWith(null);
  expect(screen.getByText(i18n.t('providerFields.originalCall'), { selector: 'code' })).toBeVisible();
});
