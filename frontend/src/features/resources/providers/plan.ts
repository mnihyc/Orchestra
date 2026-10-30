/** Native observation times, reset review eligibility and UTC ledger periods. */
import type { Provider as PlanProvider, UsageWindow as PlanWindow, ResetCredit } from '@/generated/api-types';
export type { Provider as PlanProvider, UsageWindow as PlanWindow, ResetCredit, UsageAnalytics as PlanAnalytics, ProviderResetConsumeBody as ResetBody } from '@/generated/api-types';
import { DAY_MS } from '@/lib/time';
import { detailState } from './details';

/** A source that has never been independently observed cannot borrow a different group's date. */
export function observedAt(provider: PlanProvider, field: string): string | null {
  const member = field.includes('.') ? field.slice(0, field.indexOf('.')) : field;
  return provider.facts.field_times[field] ?? provider.facts.field_times[member] ?? null;
}

/** Only a successful discovery can say that a listed model was absent; a failed read preserves its earlier date. */
export function modelUnavailableAt(provider: PlanProvider, model: string): string | null {
  const state = detailState(provider,'models');
  if (state !== 'told' && state !== 'empty') return null;
  if (state === 'empty') return observedAt(provider,'models');
  const models = provider.facts.models;
  return models === null || models.some((entry) => entry.model_name === model && entry.listed)
    ? null : observedAt(provider, 'models');
}

export function resetState(window: PlanWindow): 'known' | 'unset' | 'unknown' {
  return window.resets_at !== null ? 'known' : window.reset_not_started === true ? 'unset' : 'unknown';
}

/** A listed available credit may omit its lapse; a known expired credit is never offered. The act rereads its id, status and count. */
export function resetUsable(credit: ResetCredit, now: number): boolean {
  return credit.status === 'available' && (credit.expires_at === null || Date.parse(credit.expires_at) > now);
}

export function canonicalProviderAddress(provider: PlanProvider): string {
  return provider.address;
}

/** Daily UTC buckets let all three Overview periods share one ledger request, including early-month overlap. */
export function ledgerPeriods(now: number) {
  const date = new Date(now);
  const today = Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate());
  const sevenDays = today - 6 * DAY_MS;
  const month = Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), 1);
  return { today, sevenDays, month, from: new Date(Math.min(month, sevenDays)).toISOString().slice(0, 10), to: new Date(today + DAY_MS).toISOString().slice(0, 10) };
}

/** Minimal administrative identity is readable; ordinary content and controls are not. */
export function privateProvider(provider: Pick<PlanProvider, 'access'>): boolean {
  return !provider.access.permissions.includes('provider.view');
}

/** Distinct providers can have their usage sections open beside each other. */
export function planSectionId(id:string):string { return `usage-plan-${id}`; }

