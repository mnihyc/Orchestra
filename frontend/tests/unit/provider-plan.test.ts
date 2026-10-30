/** Native unknowns, source ages, UTC boundaries and exact reset eligibility. */
import { describe, expect, it } from 'vitest';
import { canonicalProviderAddress, ledgerPeriods, modelUnavailableAt, observedAt, resetState, resetUsable } from '@/features/resources/providers/plan';
import { OBSERVED, PLAN_NOW, planProvider } from './provider-plan-fixtures';

describe('provider observations', () => {
  it('dates each successful source independently and never borrows another endpoint’s fresh date', () => {
    const provider = planProvider();
    provider.facts.field_times = { spend: OBSERVED, balances: '2026-10-12T12:00:00.000Z' };
    expect(observedAt(provider, 'spend.key:total')).toBe(OBSERVED);
    expect(observedAt(provider, 'balances.account')).toBe('2026-10-12T12:00:00.000Z');
    expect(observedAt(provider, 'windows')).toBeNull();
  });
  it('does not interpret binding or zero use as an unstarted reset', () => {
    const window = planProvider().facts.windows[0]!;
    expect(resetState(window)).toBe('unset');
    window.reset_not_started = null;
    window.is_active = true;
    expect(resetState(window)).toBe('unknown');
  });
  it('dates each window by its own successful read or the whole window report, never by the plan', () => {
    const provider = planProvider();
    provider.facts.field_times = { plan: OBSERVED, windows: '2026-10-12T12:00:00.000Z', 'windows.weekly': OBSERVED };
    expect(observedAt(provider, 'windows.primary')).toBe('2026-10-12T12:00:00.000Z');
    expect(observedAt(provider, 'windows.weekly')).toBe(OBSERVED);
  });
  it('marks only a discovered absence with its actual read time and leaves model acceptance alone', () => {
    const provider = planProvider();
    expect(modelUnavailableAt(provider, 'old')).toBeNull();
    provider.facts.models = [{ model_name: 'new', display_name: 'New', listed: true, parameters: {} }];
    provider.facts_state.models = 'told';
    provider.facts.field_times.models = OBSERVED;
    expect(modelUnavailableAt(provider, 'old')).toBe(OBSERVED);
    expect(modelUnavailableAt(provider, 'new')).toBeNull();
    expect(provider.models).toEqual([]);
  });
  it('uses the source’s explicit empty models state and its time even when the nullable value carries no rows', () => {
    const provider=planProvider();
    provider.facts.models=null;provider.facts_state.models='empty';provider.facts.field_times.models=OBSERVED;
    expect(modelUnavailableAt(provider,'previously-enabled')).toBe(OBSERVED);
    provider.facts_state.models='not_read_yet';
    expect(modelUnavailableAt(provider,'previously-enabled')).toBeNull();
  });
  it('reviews a listed available credit without requiring an optional lapse, while refusing unknown status or a known expiry', () => {
    const credit = planProvider().facts.available_resets!.credits![0]!;
    expect(resetUsable(credit, PLAN_NOW)).toBe(true);
    expect(resetUsable({ ...credit, expires_at: null }, PLAN_NOW)).toBe(true);
    expect(resetUsable({ ...credit, status: 'unknown' }, PLAN_NOW)).toBe(false);
    expect(resetUsable({ ...credit, expires_at: OBSERVED }, PLAN_NOW)).toBe(false);
  });
  it('requests one range for UTC today, inclusive seven calendar days and this month across month boundaries', () => {
    expect(ledgerPeriods(Date.parse('2026-11-01T23:50:00Z'))).toMatchObject({ from: '2026-10-26', to: '2026-11-02', today: Date.parse('2026-11-01T00:00:00Z'), month: Date.parse('2026-11-01T00:00:00Z') });
  });
  it('uses the server’s resource address unchanged', () => {
    expect(canonicalProviderAddress(planProvider({ address: 'acme.team.native-address' }))).toBe('acme.team.native-address');
  });
});
