/** Running configuration facts stay under the current System read and never become guessed defaults. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Admin from '@/api/admin';
import { ApiError } from '@/api/client';
import { System } from '@/features/admin/System';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const response = vi.hoisted(() => ({ denied: false, calls: 0, reported: true }));
vi.mock('@/api/admin', async(original) => ({ ...await original<typeof Admin>(), systemQuery: {
  queryKey: ['system-config-test'], retry: false, queryFn: () => {
    response.calls += 1;
    if (response.denied) throw new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Access changed', detail: 'Access changed', status: 403, request_id: 'configuration-read' }, null);
    return Promise.resolve({ version: 'test-version', effective_configuration: response.reported ? {
      service: 'api', process_revision: 'opaque-process', measured_at: '2026-11-15T00:00:00Z', values: { next_cursor: null, items: [
        { public_key: 'database.acquire_timeout_ms', typed_value: { kind: 'finite', milliseconds: '10000' }, origin: 'environment', application: 'restart' },
        { public_key: 'database.idle_transaction_timeout_ms', typed_value: { kind: 'indefinite' }, origin: null, application: 'restart' },
      ] },
    } : undefined });
  },
} }));
vi.mock('@/features/admin/Installation', () => ({ Installation: () => null }));
beforeEach(() => { response.denied=false; response.calls=0; response.reported=true; });
afterEach(async() => { cleanup(); await setLocale('en'); });

it.each(['en','zh-CN'] as const)('shows captured running origins, keeps missing origins unreported and releases them after refusal in %s', async(locale) => {
  await setLocale(locale);
  const cache=new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view=show(<System/>,cache);
  try {
    await screen.findByText('test-version');
    expect(screen.queryByText(locale==='en'?'Environment override':'环境变量覆盖')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button',{name:i18n.t('common.details')}));
    expect(await screen.findByText(locale==='en'?'Environment override':'环境变量覆盖')).toBeVisible();
    expect(screen.getByText(locale==='en'?'Not reported':'尚未报告')).toBeVisible();
    expect(screen.getByText(locale==='en'?'No deadline':'无截止时间')).toBeVisible();
    expect(response.calls).toBe(1);
    response.denied=true;
    await act(async()=>{ await cache.invalidateQueries({queryKey:['system-config-test']}); });
    await waitFor(()=>expect(screen.queryByText(locale==='en'?'Environment override':'环境变量覆盖')).not.toBeInTheDocument());
    expect(screen.queryByText('opaque-process')).not.toBeInTheDocument();
  } finally { view.unmount(); cache.clear(); }
});

it('keeps an old process without captured configuration unreported',async()=>{
  response.reported=false;show(<System/>);
  await screen.findByText('test-version');
  await userEvent.click(screen.getByRole('button',{name:i18n.t('common.details')}));
  expect(await screen.findByText('Not reported')).toBeVisible();
  expect(screen.queryByText('Compiled default')).not.toBeInTheDocument();
  expect(response.calls).toBe(1);
});
