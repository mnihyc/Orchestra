/** Provider details follow the API's member states and source declarations, independently of the credential kind. */
import type { Provider } from '@/generated/api-types';

/** The vendor's named window, qualified only when it supplies a different model-family name. */
export function windowName(window: { label: string; model_family?: string | null }): string {
  return window.model_family === null || window.model_family === undefined || window.model_family === '' || window.model_family === window.label ? window.label : `${window.label} (${window.model_family})`;
}

/** The card's fixed reading order; every key is an ordinary facts member. */
export const FACT_MEMBERS = ['identity', 'credential', 'credential_verified', 'plan', 'windows', 'limit_reached', 'ordinary_usage_allowed', 'balances', 'spend', 'extra_usage', 'available_resets'] as const;
export type DetailMember = (typeof FACT_MEMBERS)[number];
export type DetailState = 'unsupported' | 'not_read_yet' | 'empty' | 'told';
export type Balance = Provider['facts']['balances'][number];
export type Spend = Provider['facts']['spend'][number];

/** Only the source's explicit member state decides whether a block is supported, unread or empty. */
export function detailState(provider: Pick<Provider, 'facts_state'>, member: DetailMember | 'models'): DetailState {
  return memberState(provider.facts_state[member]);
}

/** A reason qualifies unsupported; it never becomes a read-in-progress or a fabricated amount. */
export function memberState(value: unknown): DetailState {
  return value === 'told' || value === 'empty' || value === 'not_read_yet' ? value : 'unsupported';
}

export function unavailableReason(provider: Provider, member: DetailMember | 'models'): string | null {
  const value: unknown = provider.facts_state[member];
  if (typeof value !== 'object' || value === null || !('unsupported_because' in value)) return null;
  const detail = value.unsupported_because;
  return typeof detail === 'object' && detail !== null && 'reason' in detail && typeof detail.reason === 'string' ? detail.reason : null;
}

/** Collections accept the contract's explicit empty and nullable forms without manufacturing a zero amount. */
export function reportedList<T>(value: readonly T[] | null | undefined): readonly T[] { return value ?? []; }

export function factsSource(provider: Provider) { return provider.sources.facts; }
export function modelsSource(provider: Provider) { return provider.sources.models; }
export function spendsResets(provider: Provider): boolean { return factsSource(provider).descriptor?.facts?.spends_resets === true; }

/** No authentication is a declared endpoint option; an absent credential alone never says it is unnecessary. */
export function endpointsNeedNoKey(provider: Pick<Provider, 'upstreams'>): boolean {
  return provider.upstreams.length > 0 && provider.upstreams.every(({ options }) =>
    options !== null && typeof options === 'object' && !Array.isArray(options) && 'auth' in options && options.auth === 'none');
}

/** Decimal text stays text so that a source's precision is not rounded through a JavaScript number. */
export function amountText(value: string | null | undefined, unit: string, unknown: string): string {
  return value === null || value === undefined ? unknown : `${value} ${unit}`;
}

/** The shared contract resolves field support; absent metadata preserves the earlier unknown display. */
export function detailFieldVisible(provider: Provider, path: string): boolean {
  const state = provider.facts_state.fields?.[path];
  return state === undefined || memberState(state) !== 'unsupported';
}

/** A metric hides only when every known amount is explicitly unsupported; unresolved groups remain unknown. */
export function creditHistoryVisible(provider:Provider):boolean {
  const fields=provider.facts_state.fields ?? {};
  for(const member of ['balances','spend','extra_usage'] as const){
    if(detailState(provider,member)==='unsupported')continue;
    const quantities=member==='balances' ? ['remaining','total','used','unlimited'] : member==='spend' ? ['used','limit','remaining'] : ['used','limit'];
    const groups=member==='extra_usage' ? new Set(['extra_usage']) : new Set([
      ...provider.facts[member].map((item)=>`${member}.${item.id}`),
      ...Object.keys(fields).filter((path)=>path.startsWith(`${member}.`)).map((path)=>path.slice(0,path.lastIndexOf('.'))),
    ]);
    if(groups.size===0 || [...groups].some((group)=>quantities.some((quantity)=>fields[`${group}.${quantity}`]!=='unsupported')))return true;
  }
  return false;
}

/** Moving a decimal point keeps every reported digit. Huge scales stay explicit without unbounded padding. */
export function scaledDecimal(value: string, places: number | null | undefined): string {
  const scale=places ?? 0;
  if(scale===0)return value;
  // The amount contract permits at most forty characters. Beyond its textual precision, a power is clearer and bounded.
  if(scale>40)return `${value} × 10^−${scale}`;
  const negative=value.startsWith('-');
  const [whole='',fraction='']=(negative ? value.slice(1) : value).split('.');
  const digits=(whole+fraction).padStart(fraction.length+scale+1,'0');
  const point=digits.length-fraction.length-scale;
  const integer=digits.slice(0,point).replace(/^0+(?=\d)/,'');
  const decimal=digits.slice(point).replace(/0+$/,'');
  return `${negative ? '-' : ''}${integer}${decimal==='' ? '' : `.${decimal}`}`;
}

export function scaledAmountText(value:string | null | undefined,unit:string | null,places:number | null | undefined,unknown:string,unitUnknown:string):string {
  return value===null || value===undefined ? unknown : `${scaledDecimal(value,places)} ${unit ?? unitUnknown}`;
}
