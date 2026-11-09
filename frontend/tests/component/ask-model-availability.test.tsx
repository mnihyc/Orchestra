/** Native blocked reasons retire the visible composer while its local lifetime and Stop remain. */
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import { focusManager } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { AskModelAvailability } from '@/features/ask/ModelAvailability';
import { i18n, setLocale } from '@/i18n';
import { askModels } from '../unit/ask-fixtures';
import { org } from '../doubles/fixtures';
import { ADA } from '../unit/general-access-fixtures';
import { show } from './support';
import type { AskModelsReply } from '@/generated/api-types';
let reply: AskModelsReply;
const mounted = vi.fn();
vi.mock('@/api/ask', () => ({ askModelsQuery: (root: string) => ({ queryKey: ['ask-availability-test', root], queryFn: async () => reply }) }));
vi.mock('@/api/nodes', async (load) => ({ ...await load<typeof import('@/api/nodes')>(),
  nodeQuery: (id: string) => ({ queryKey: ['availability-org', id], queryFn: async () => org(id, 'acme', 'Acme', ADA) }),
}));
function Draft({ available }: { available: boolean }) {
  const [value, setValue] = useState(''); useEffect(() => { mounted(); }, []);
  return <input aria-label="Question draft" disabled={!available} value={value} onChange={(event) => setValue(event.target.value)} />;
}
afterEach(async () => { cleanup(); mounted.mockClear(); focusManager.setFocused(undefined); await setLocale('en'); });

const reasons = [
  ['not_configured', 'ask.blockedNotConfigured'], ['empty', 'ask.blockedEmpty'],
  ['no_usable_models', 'ask.blockedNoUsableModels'], ['excluded_by_policy', 'ask.blockedByPolicy'],
] as const;
for (const locale of ['en', 'zh-CN'] as const) {
  it.each(reasons)(`shows native %s without an unusable composer or unauthorized link (${locale})`, async (reason, key) => {
    await setLocale(locale); reply = askModels('root', { models: [], default_model: null, blocked_reason: reason, writable: false });
    show(<AskModelAvailability root="root">{(available) => <Draft available={available} />}</AskModelAvailability>);
    expect(await screen.findByText(i18n.t(key))).toBeVisible();
    expect(screen.queryByRole('textbox', { name: 'Question draft' })).not.toBeInTheDocument();
    expect(screen.queryByRole('link', { name: i18n.t('ask.configureModels') })).not.toBeInTheDocument();
  });
  it(`shows the exact settings destination only with native writability (${locale})`, async () => {
    await setLocale(locale); reply = askModels('root', { models: [], default_model: null, blocked_reason: 'empty', writable: true });
    show(<AskModelAvailability root="root">{(available) => <Draft available={available} />}</AskModelAvailability>);
    expect(await screen.findByRole('link', { name: i18n.t('ask.configureModels') })).toHaveAttribute('href', '/org/acme/settings/policies?setting=ask.models');
  });
  it.each([['no_use', 'ask.modelExclusion_no_use'], ['parameters_unavailable', 'ask.modelExclusion_parameters_unavailable']] as const)(`keeps a refused configured choice's %s reason (${locale})`, async (reason, key) => {
    await setLocale(locale); const configured = askModels('root').models[0];
    if (configured === undefined) throw new Error('The configured native entry is required.');
    reply = askModels('root', { default_model: null, blocked_reason: 'no_usable_models', models: [{ ...configured, allowed: false, is_default: false, unavailable_reason: reason }] });
    show(<AskModelAvailability root="root">{(available) => <Draft available={available} />}</AskModelAvailability>);
    expect(await screen.findByText(`${i18n.t('ask.blockedNoUsableModels')} · ${i18n.t(key)}`)).toBeVisible();
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  });
  it(`preserves a draft and its mount when availability changes and keeps Stop independent (${locale})`, async () => {
    await setLocale(locale); reply = askModels('root');
    const stop = vi.fn();
    show(<AskModelAvailability root="root" stop={<button onClick={stop}>{i18n.t('composer.stop')}</button>}>{(available) => <Draft available={available} />}</AskModelAvailability>);
    const field = await screen.findByRole('textbox', { name: 'Question draft' }); fireEvent.change(field, { target: { value: 'Keep this local question' } });
    reply = askModels('root', { models: [], default_model: null, blocked_reason: 'empty' });
    focusManager.setFocused(false); focusManager.setFocused(true);
    await screen.findByText(i18n.t('ask.blockedEmpty')); expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: i18n.t('composer.stop') })); expect(stop).toHaveBeenCalledOnce();
    reply = askModels('root'); focusManager.setFocused(false); focusManager.setFocused(true);
    await waitFor(() => expect(screen.getByRole('textbox', { name: 'Question draft' })).toBeVisible());
    expect(screen.getByRole('textbox', { name: 'Question draft' })).toBe(field); expect(field).toHaveValue('Keep this local question');
    expect(mounted).toHaveBeenCalledOnce();
  });
}
