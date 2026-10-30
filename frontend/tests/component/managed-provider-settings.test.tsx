/** A private infrastructure provider never leaks its model or creates an error state in a resolved setting. */
import { screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { SettingSummary } from '@/features/settings/SettingSummary';
import { i18n } from '@/i18n';
import { show } from './support';
const calls = vi.hoisted(() => ({read: vi.fn()}));
vi.mock('@/api/resources', async (original) => ({...await original<Record<string, unknown>>(), providerQuery: (id: string) => ({queryKey: ['private-provider', id], queryFn: calls.read})}));
afterEach(() => vi.clearAllMocks());
it.each([['auto.summarizer', 403], ['auto.summarizer', 404], ['search.semantic.embeddings', 403], ['search.semantic.embeddings', 404], ['auto.summarizer', null], ['search.semantic.embeddings', null]] as const)('shows only Managed for %s when its provider answers %s', async (key, status) => {
  if (status === null) calls.read.mockResolvedValue(null);
  else calls.read.mockRejectedValue(new ApiError({type: 'about:blank', title: 'Private', status, code: status === 403 ? 'authz.denied' : 'not_found', detail: 'Private', request_id:'private-provider'}, null));
  const view = show(<SettingSummary def={{key, title: 'Embeddings'}} value={{kind: 'model', provider_id: 'private-provider', model_name: 'private-model', dimensions: 256}} />);
  expect(await screen.findByText(i18n.t('settings.compact.managed'))).toHaveClass('text-text-muted');
  expect(view.container).not.toHaveTextContent('private-model');
  expect(view.container).not.toHaveTextContent('256');
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  view.unmount();
});
