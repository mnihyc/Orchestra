/** Secret names supplied by an opening remain visible; absent metadata waits for a deliberate reader. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState, type ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { client } from '@/api/client';
import { secretsKey } from '@/api/settings';
import { Secrets } from '@/features/settings/Secrets';
import type { RowPlace } from '@/features/settings/SettingRow';
import type { SecretInfo } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const read = vi.hoisted(() => vi.fn());
vi.mock('@/api/client', async (load) => { const actual = await load<typeof Client>(); return { ...actual, api: { ...actual.api, GET: read } }; });
const saved = client.session.getState(), caches: QueryClient[] = [];
const target = { kind: 'node', id: 'secret-topic' } as const;
const place: RowPlace = { name: 'Chosen topic', kind: 'topic', owner: null, access: { permissions: ['settings.manage'] } };
const secret: SecretInfo = { name: 'DEPLOY_TOKEN', node: { id: target.id, kind: 'topic', name: place.name, redacted: false }, rotated_at: null };
function cache() { const held = new QueryClient({ defaultOptions: { queries: { retry: false } } }); caches.push(held); return held; }
function draw(content: ReactNode, held = cache()) { return show(<QueryClientProvider client={held}>{content}</QueryClientProvider>); }
beforeEach(() => { client.session.setState({ userId: 'secret-reader', readerVersion: 1 }); read.mockReset(); read.mockResolvedValue({ data: [secret], response: new Response() }); });
afterEach(async () => { cleanup(); for (const held of caches.splice(0)) held.clear(); client.session.setState(saved); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) {
  it(`opens the actual names reader only when asked and retains replacement and removal (${locale})`, async () => {
    await setLocale(locale); draw(<Secrets target={target} place={place} />);
    expect(read).not.toHaveBeenCalled(); expect(screen.queryByText(i18n.t('settings.secrets.none'))).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('settings.secrets.showNames') }));
    expect(await screen.findByText(secret.name)).toBeVisible(); expect(read).toHaveBeenCalledTimes(1);
    expect(read).toHaveBeenCalledWith('/nodes/{id}/secrets', expect.objectContaining({ params: { path: { id: target.id } } }));
    expect(screen.getByRole('button', { name: i18n.t('settings.secrets.replaceNamed', { name: secret.name }) })).toBeEnabled();
    expect(screen.getByRole('button', { name: i18n.t('settings.secrets.removeNamed', { name: secret.name }) })).toBeEnabled();
  });
  it(`keeps supplied names without a second read and opens the existing secret form (${locale})`, async () => {
    await setLocale(locale); const held = cache(); held.setQueryData(secretsKey(target), [secret]); draw(<Secrets target={target} place={place} />, held);
    expect(screen.getByText(secret.name)).toBeVisible(); expect(read).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('settings.secrets.add') }));
    expect(await screen.findByRole('dialog', { name: i18n.t('settings.secrets.newTitle', { place: place.name }) })).toBeVisible();
    await waitFor(() => expect(read).toHaveBeenCalledTimes(1)); expect(screen.getByLabelText(i18n.t('settings.secrets.value'))).toHaveValue('');
  });
  it(`keeps an explicitly supplied empty list truthful without another read (${locale})`, async () => {
    await setLocale(locale); const held = cache(); held.setQueryData(secretsKey(target), []); draw(<Secrets target={target} place={place} />, held);
    expect(screen.getByText(i18n.t('settings.secrets.none'))).toBeVisible(); expect(read).not.toHaveBeenCalled();
  });
  it(`preserves read-only secret permissions (${locale})`, async () => {
    await setLocale(locale); const held = cache(); held.setQueryData(secretsKey(target), [secret]); draw(<Secrets target={target} place={{ ...place, access: { permissions: [] } }} />, held);
    expect(screen.getByText(secret.name)).toBeVisible(); const add = screen.getByRole('button', { name: i18n.t('settings.secrets.add') }); expect(add).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(add); expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: i18n.t('settings.secrets.replaceNamed', { name: secret.name }) })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: i18n.t('settings.secrets.removeNamed', { name: secret.name }) })).not.toBeInTheDocument(); expect(read).not.toHaveBeenCalled();
  });
  it(`ends an opened names reader when the settings target changes (${locale})`, async () => {
    await setLocale(locale);
    function Targets() {
      const [id, setId] = useState<string>(target.id);
      return <><button onClick={() => setId('other-topic')}>Other topic</button><Secrets target={{ kind: 'node', id }} place={place} /></>;
    }
    draw(<Targets />); await userEvent.click(screen.getByRole('button', { name: i18n.t('settings.secrets.showNames') }));
    await screen.findByText(secret.name); expect(read).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole('button', { name: 'Other topic' }));
    expect(screen.getByRole('button', { name: i18n.t('settings.secrets.showNames') })).toBeVisible(); expect(read).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('settings.secrets.showNames') }));
    await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
    expect(read.mock.calls[1]?.[1]).toMatchObject({ params: { path: { id: 'other-topic' } } });
  });
}
