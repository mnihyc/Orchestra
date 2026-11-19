/** A private infrastructure provider never leaks its model or creates an error state in a resolved setting. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import type * as Client from '@/api/client';
import { providerKey } from '@/api/resources';
import { SettingSummary } from '@/features/settings/SettingSummary';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';
const calls = vi.hoisted(() => ({read: vi.fn()}));
vi.mock('@/api/client', async (load) => { const actual = await load<typeof Client>(); return { ...actual, api: { ...actual.api, GET: calls.read } }; });
const caches: QueryClient[] = [];
function nativeNames() {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } }); caches.push(cache); return cache;
}
afterEach(async () => { cleanup(); for (const cache of caches.splice(0)) cache.clear(); vi.clearAllMocks(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) {
  it.each([['auto.summarizer', 403], ['auto.summarizer', 404], ['search.semantic.embeddings', 403], ['search.semantic.embeddings', 404], ['auto.summarizer', null], ['search.semantic.embeddings', null]] as const)(`shows only Managed for %s when its supplied provider name answers %s (${locale})`, async (key, status) => {
    await setLocale(locale);
    const cache = nativeNames(), queryKey = providerKey('private-provider');
    // A closed summary consumes the native name result without starting another GET.
    if (status === null) cache.setQueryData(queryKey, null);
    else cache.getQueryCache().build(cache, { queryKey }).setState({ status: 'error', data: undefined,
      error: new ApiError({type: 'about:blank', title: 'Private', status, code: status === 403 ? 'authz.denied' : 'not_found', detail: 'Private', request_id:'private-provider'}, null) });
    const view = show(<QueryClientProvider client={cache}><SettingSummary def={{key, title: 'Embeddings'}} value={{kind: 'model', provider_id: 'private-provider', model_name: 'private-model', dimensions: 256}} /></QueryClientProvider>);
    expect(await screen.findByText(i18n.t('settings.compact.managed'))).toHaveClass('text-text-muted');
    expect(view.container).not.toHaveTextContent('private-model');
    expect(view.container).not.toHaveTextContent('256');
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(calls.read).not.toHaveBeenCalled();
    view.unmount();
  });
  it.each(['auto.summarizer', 'search.semantic.embeddings'] as const)(`keeps supplied model facts while its provider name is unread for %s (${locale})`, async (key) => {
    await setLocale(locale); const cache = nativeNames();
    const view = show(<QueryClientProvider client={cache}><SettingSummary def={{key, title: 'Embeddings'}} value={{kind: 'model', provider_id: 'unread-provider', model_name: 'Supplied model', dimensions: 256}} /></QueryClientProvider>);
    // The summarizer reads as one sentence (a model, its provider and its model); the embeddings keep their fields.
    if (key === 'auto.summarizer') expect(view.container).toHaveTextContent(i18n.t('places.providerDefault.selected'));
    else { expect(screen.getByText(i18n.t('places.providerDefault.selected'))).toBeVisible(); expect(view.container).toHaveTextContent('256'); }
    expect(view.container).toHaveTextContent('Supplied model');
    expect(view.container).not.toHaveTextContent('unread-provider');
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    await act(async () => { await Promise.resolve(); }); expect(calls.read).not.toHaveBeenCalled();
  });
}
