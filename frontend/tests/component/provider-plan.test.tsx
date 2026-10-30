/** Matched provider cards preserve zero, family, missing data, overage and privacy. */
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useQuery } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AnchorHTMLAttributes,ReactNode } from 'react';
import type * as RouterModule from '@tanstack/react-router';
import { providerQuery, refreshProvider } from '@/api/resources';
import type * as ResourcesApi from '@/api/resources';
import { ModelAvailability } from '@/features/resources/providers/Models';
import { PlanPanel } from '@/features/resources/providers/PlanPanel';
import { ProviderUsageCard } from '@/features/resources/providers/ProviderUsageCard';
import { OBSERVED, planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

vi.mock('@tanstack/react-router',async(load)=>({...await load<typeof RouterModule>(),Link:({to,params,search,hash,children,...anchor}:AnchorHTMLAttributes<HTMLAnchorElement> & {to:string;params?:Record<string,string>;search?:Record<string,string>;hash?:string;children?:ReactNode})=><a {...anchor} href={`${to.replace(/\$(\w+)/g,(_,key:string)=>params?.[key] ?? '')}?${new URLSearchParams(search)}${hash===undefined ? '' : `#${hash}`}`}>{children}</a>}));
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof ResourcesApi>(), refreshProvider: vi.fn() }));
afterEach(() => { vi.clearAllMocks(); vi.restoreAllMocks(); });

function RefreshPanel({ provider }: { provider: ReturnType<typeof planProvider> }) {
  const read = useQuery({ ...providerQuery(provider.id), initialData: provider, enabled: false });
  return <PlanPanel provider={read.data} />;
}

describe('one provider usage card', () => {
  it('keeps a real zero and native family labels without a warning mark', () => {
    show(<ProviderUsageCard provider={planProvider()} />);
    expect(screen.getByRole('meter', { name: '5-hour window' })).toHaveAttribute('aria-valuenow', '0');
    expect(screen.getByText('Reset time not set')).toBeInTheDocument();
    expect(screen.getByRole('meter', { name: 'Weekly window (Fable)' })).toHaveAttribute('aria-valuenow', '81');
    expect(screen.queryByText(/warning/i)).not.toBeInTheDocument();
  });
  it('preserves reported overage in its amount and caps the visual track only', () => {
    const provider = planProvider();
    provider.facts.windows[1]!.used_percent = 123.4;
    show(<ProviderUsageCard provider={provider} />);
    expect(screen.getByRole('meter', { name: 'Weekly window (Fable)' })).toHaveAttribute('aria-valuetext', '123.4%');
    expect(screen.getByRole('meter', { name: 'Weekly window (Fable)' })).toHaveAttribute('aria-valuenow', '100');
  });
  it('keeps a known empty source distinct from never reported', () => {
    const provider = planProvider();
    provider.facts.windows = [];
    provider.facts_state.windows = 'empty';
    const { rerender } = show(<ProviderUsageCard provider={provider} />);
    expect(screen.getByText('No usage windows')).toBeInTheDocument();
    provider.facts.field_times = {};
    provider.facts_state.windows = 'not_read_yet';
    rerender(<ProviderUsageCard provider={structuredClone(provider)} />);
    expect(screen.queryByText('No usage windows')).not.toBeInTheDocument();
    const unread = screen.getByText(/^Not read yet:/);
    expect(unread).toHaveTextContent('Usage windows');
    expect(screen.getAllByText(/^Not read yet:/)).toHaveLength(1);
    expect(screen.queryByRole('meter')).not.toBeInTheDocument();
  });
  it('offers only binding windows in their native order in a compact choice', () => {
    const provider = planProvider();
    const { rerender } = show(<ProviderUsageCard compact provider={provider} />);
    expect(screen.getByText('Weekly window (Fable) 81% used')).toBeInTheDocument();
    expect(screen.queryByText(/5-hour/)).not.toBeInTheDocument();
    provider.access.permissions = ['provider.use'];
    rerender(<ProviderUsageCard compact provider={provider} />);
    expect(screen.getByText('Room not visible')).toBeInTheDocument();
    expect(screen.queryByText(/81%/)).not.toBeInTheDocument();
  });
  it('keeps known spending when the balance read has told nothing yet', () => {
    const provider = planProvider({ kind: 'api' });
    const total = {id:'key:total',label:'Total',unit:'USD',period:'total' as const,period_minutes:null,used:'20',limit:null,remaining:'12.4',resets_at:null};
    provider.facts.spend = [total,{...total,id:'key:day',period:'day',used:'0',remaining:null},{...total,id:'key:week',period:'week',used:'5',remaining:null},{...total,id:'key:month',period:'month',used:'10',remaining:null}];
    provider.facts_state.spend = 'told';
    provider.facts.balances = [];
    provider.facts_state.balances = 'not_read_yet';
    show(<ProviderUsageCard provider={provider} />);
    expect(screen.getByText(/12\.4 USD remaining/)).toBeInTheDocument();
    expect(screen.getByText(/^0 USD used of /)).toBeInTheDocument();
    const unread = screen.getByText(/^Not read yet:/);
    expect(unread).toHaveTextContent('Balance');
    expect(screen.getAllByText(/^Not read yet:/)).toHaveLength(1);
  });
  it('retains the vendor’s decimal scale and unit for extra spending', () => {
    const provider = planProvider();
    provider.facts.extra_usage = { enabled: true, used: 1234, limit: 2000, used_percent: 61.7, unit: 'GBP', decimal_places: 2, disabled_reason: null, limit_reached: false };
    provider.facts_state.extra_usage = 'told';
    show(<ProviderUsageCard provider={provider} />);
    expect(screen.getByText('12.34 GBP used of 20 GBP')).toBeInTheDocument();
  });
  it('shows no card for administrative existence without View', () => {
    const provider = planProvider();
    provider.access.permissions = [];
    const shown = show(<ProviderUsageCard provider={provider} />);
    expect(shown.container.querySelector('[data-provider-usage]')).not.toBeInTheDocument();
    expect(screen.queryByRole('meter')).not.toBeInTheDocument();
  });
  it('puts each actual source time beside the plan, each window and each balance', () => {
    vi.spyOn(Date, 'now').mockReturnValue(Date.parse('2026-10-14T12:01:00.000Z'));
    const provider = planProvider();
    const { container, unmount } = show(<ProviderUsageCard provider={provider} />);
    for (const field of ['plan', 'windows.primary', 'windows.weekly']) {
      const line = container.querySelector(`[data-source="${field}"]`);
      expect(line).toHaveTextContent('Read');
      expect(line).toHaveTextContent(/2 min/);
      expect(line?.querySelector('time')).toHaveAttribute('datetime', OBSERVED);
    }
    unmount();
    const key = planProvider({ kind: 'api' });
    key.facts.field_times = { spend: OBSERVED, balances: '2026-10-12T12:00:00.000Z' };
    const drawn = show(<ProviderUsageCard provider={key} />);
    expect(drawn.container.querySelector('[data-source="balances.plan"] time')).toHaveAttribute('datetime', '2026-10-12T12:00:00.000Z');
  });
  it('shows Refresh reading and then the new source time from its response', async () => {
    let finish: (provider: ReturnType<typeof planProvider>) => void = () => undefined;
    vi.mocked(refreshProvider).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const provider = planProvider();
    const { container } = show(<RefreshPanel provider={provider} />);
    await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    expect(await screen.findByRole('button', { name: 'Reading…' })).toHaveAttribute('aria-disabled', 'true');
    const fresh = structuredClone(provider);
    fresh.revision += 1;
    fresh.facts.field_times.plan = '2026-10-14T12:02:00.000Z';
    await act(async () => { finish(fresh); await Promise.resolve(); });
    await waitFor(() => expect(container.querySelector('[data-source="plan"] time')).toHaveAttribute('datetime', '2026-10-14T12:02:00.000Z'));
    expect(container.querySelector('[data-source="windows.primary"] time')).toHaveAttribute('datetime', OBSERVED);
    expect(screen.getByRole('button', { name: 'Refresh' })).toBeEnabled();
  });
  it('marks a gone model with when it was seen without switching it off', () => {
    const provider = planProvider();
    provider.facts.models = [];
    provider.facts_state.models = 'empty';
    provider.facts.field_times.models = OBSERVED;
    const { container } = show(<ModelAvailability provider={provider} name="old-model" />);
    expect(screen.getByText(/No longer offered/)).toBeInTheDocument();
    expect(container.querySelector('time')).toHaveAttribute('datetime', OBSERVED);
  });
});
