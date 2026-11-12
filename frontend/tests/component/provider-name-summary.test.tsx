/** Closed summaries consume native protected names without starting a second name reader. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, screen } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { client } from '@/api/client';
import { providerDefaultKey } from '@/api/defaultsOpening';
import { providerKey } from '@/api/resources';
import { ProviderDefaultName } from '@/features/settings/ProviderChoice';
import { i18n, setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

const reads = vi.hoisted(() => vi.fn());
vi.mock('@/api/client', async (load) => { const actual = await load<typeof Client>(); return { ...actual, api: { ...actual.api, GET: reads } }; });
const saved = client.session.getState(), caches: QueryClient[] = [], native = planProvider({ name: 'Protected current provider' });
function cache() { const held = new QueryClient({ defaultOptions: { queries: { retry: false } } }); caches.push(held); return held; }
function draw(content: ReactNode, held = cache()) { return show(<QueryClientProvider client={held}>{content}</QueryClientProvider>); }
beforeEach(() => { reads.mockClear(); client.session.setState({ userId: 'name-reader', readerVersion: 1 }); });
afterEach(async () => { cleanup(); for (const held of caches.splice(0)) held.clear(); client.session.setState(saved); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps an unread closed name honest and opaque without a request (${locale})`, async () => {
    await setLocale(locale); const view = draw(<ProviderDefaultName value={native.id} />);
    expect(screen.getByText(i18n.t('places.providerDefault.selected'))).toBeVisible(); expect(view.container).not.toHaveTextContent(native.id);
    await act(async () => { await Promise.resolve(); }); expect(reads).not.toHaveBeenCalled();
  });
  for (const source of ['choice', 'reference'] as const) it(`uses an already authorized ${source} name without another read (${locale})`, async () => {
    await setLocale(locale); const held = cache(); held.setQueryData(source === 'choice' ? providerDefaultKey(native.id) : providerKey(native.id), native);
    draw(<ProviderDefaultName value={native.id} />, held);
    expect(screen.getByText(native.name)).toBeVisible(); await act(async () => { await Promise.resolve(); }); expect(reads).not.toHaveBeenCalled();
  });
  it(`keeps a native name withdrawal stronger than an unrelated rich cached row (${locale})`, async () => {
    await setLocale(locale); const held = cache(); held.setQueryData(providerKey(native.id), native); held.setQueryData(providerDefaultKey(native.id), null);
    draw(<ProviderDefaultName value={native.id} />, held);
    expect(screen.getByText(i18n.t('settings.compact.managed'))).toBeVisible(); expect(screen.queryByText(native.name)).not.toBeInTheDocument(); expect(reads).not.toHaveBeenCalled();
  });
  it(`keeps supplied model facts visible while its provider name has not been read (${locale})`, async () => {
    await setLocale(locale); draw(<ProviderDefaultName value={native.id}>{(name) => <span>{name} · Supplied model · 1536</span>}</ProviderDefaultName>);
    expect(screen.getByText(/Supplied model · 1536/u)).toBeVisible(); expect(reads).not.toHaveBeenCalled();
  });
}
