/** What a provider is and how it stands, as one phrase each: "Subscription · Pro", "API key · 2 endpoints", "Needs sign-in", "Limit reached · resets 14:00", with what to do about it.
 */
import type { TFunction } from 'i18next';

import type { BindingProviderSetup, Provider, ProviderListRow } from '@/generated/api-types';
import { DAY_MS, formatTimeOfDay, type TimeContext } from '@/lib/time';
import type { DotTone } from '@/ui/StatusDot';
import { detailState, endpointsNeedNoKey, factsSource } from './details';
import { limitReachedNow } from '@/features/usage/ForecastLine';

/** As shipped, how many days before a token lapses its provider says so (`providers.expiry_notice_days`). */
export const EXPIRY_NOTICE_DAYS = 14;


/** What a provider's page can point to as the next step of setting it up. */
export type NextStep = 'key' | 'models' | 'signIn' | 'signInAgain' | 'runtime' | 'settings' | null;

/** The same next step is offered by the overview, its heading and the provider's list row. */
export const PROVIDER_NEXT = {
  key: { tab: 'key', label: 'resources.act.nextKey' },
  models: { tab: 'models', label: 'resources.act.nextModels' },
  signInAgain: { tab: 'key', label: 'resources.act.signInAgain' },
  signIn: { tab: 'key', label: 'resources.act.nextSignIn' },
  runtime: { tab: 'runtime', label: 'providers.overview.nextRuntime' },
  settings: { tab: 'settings', label: 'providers.overview.nextSettings' },
} as const;

export interface ProviderState {
  tone: DotTone;
  /** One phrase with what to do about it. */
  text: string;
  /** What is missing, beside the act that addresses it. */
  detail?: string;
  /** Where on the provider's page the next step is, for whoever manages it. */
  next: NextStep;
}

/** Whether a provider has a model that may be chosen. */
export const hasEnabledModel = (provider: Pick<Provider, 'models'> | Pick<ProviderListRow, 'enabled_models_count'>): boolean => 'models' in provider ? provider.models.some((model) => model.enabled) : provider.enabled_models_count > 0;

/** The days left before the provider's token lapses, when it has one that lapses within the notice. */
export function daysUntilLapse(provider: Pick<Provider, 'facts'>, now: number): number | null {
  const ends = provider.facts.credential?.expires_at;
  if (ends === null || ends === undefined) return null;
  const left = Math.ceil((Date.parse(ends) - now) / DAY_MS);
  return left <= EXPIRY_NOTICE_DAYS ? Math.max(left, 0) : null;
}

/** When a provider can take a turn again, while a window of its whole plan is used up (its card says "Limit reached"):
 * the latest reset of those windows. A window of one family of models leaves the others, and extra usage that is on and
 * has room carries turns past the window. */
export function planLimitUntil(provider: Pick<Provider, 'facts'> & { facts_at?: string | null | undefined }, now: number): string | null {
  const extra = provider.facts.extra_usage;
  if (extra !== null && extra.enabled && extra.limit_reached !== true) return null;
  let until: string | null = null;
  for (const window of provider.facts.windows) {
    if (window.model_family !== null || window.resets_at === null) continue;
    // When the window was read, as its card takes it.
    const read = provider.facts.field_times[`windows.${window.id}`] ?? provider.facts.field_times.windows ?? provider.facts_at ?? null;
    if (!limitReachedNow(window.used_percent, window.resets_at, read, now)) continue;
    if (until === null || Date.parse(window.resets_at) > Date.parse(until)) until = window.resets_at;
  }
  return until;
}

export function providerState(provider: Provider | ProviderListRow, t: TFunction, context: TimeContext, now: number): ProviderState {
  const machine = provider.machine?.name ?? '';
  switch (provider.status) {
    case 'disabled':
      return { tone: 'off', text: t('providers.phrase.disabled'), next: null };
    case 'error':
      return { tone: 'danger', text: provider.status_detail ?? t('providers.phrase.error'), next: null };
    case 'offline':
      return { tone: 'idle', text: t('providers.phrase.offline', { machine }), next: null };
    case 'exhausted': {
      const resets = provider.facts.limit_reached?.resets_at;
      return {
        tone: 'warning',
        text: resets === null || resets === undefined ? t('providers.phrase.limit') : t('providers.phrase.limitResets', { time: formatTimeOfDay(resets, context, now) }),
        next: null,
      };
    }
    case 'needs_setup': {
      const containerDown = provider.placement === 'managed' && (provider.runtime?.state === 'stopped' || provider.runtime?.state === 'failed' || provider.runtime === null && provider.machine === null);
      const reason = provider.status_detail?.trim() ?? '';
      // Both provisioning and runtime recovery can report a failed start before a runtime row exists.
      const startEnded = reason.startsWith('its runtime could not be made') || reason === 'making its runtime was cancelled';
      if (containerDown && startEnded) {
        return { tone: 'warning', text: t('providers.phrase.needsRuntime'), detail: reason, next: 'runtime' };
      }
      const credential = provider.facts.credential;
      const signedIn = credential?.yields === 'login' || credential?.yields === 'token';
      const ends = credential?.expires_at;
      const expired = ends !== null && ends !== undefined && Date.parse(ends) <= now;
      if (provider.kind === 'subscription' && expired && (credential?.yields === 'token' || credential?.yields === 'key')) return { tone: 'warning', text: t('providers.phrase.signInLapsed'), next: 'signInAgain' };
      if (provider.kind === 'subscription' && (!signedIn || expired)) return { tone: 'warning', text: t('providers.phrase.needsSignIn'), detail: t('providers.overview.needsSignInLead'), next: 'signIn' };
      if (provider.kind === 'api' && !('upstreams' in provider ? endpointsNeedNoKey(provider) : provider.no_key_needed) && !provider.key_set && provider.key_machine === null) return { tone: 'warning', text: t('providers.phrase.needsKey'), detail: t('providers.overview.needsKeyLead'), next: 'key' };
      if (!hasEnabledModel(provider)) return { tone: 'warning', text: t('providers.phrase.needsModel'), detail: t('providers.overview.needsModelLead'), next: 'models' };
      if (containerDown) return { tone: 'warning', text: t('providers.phrase.needsRuntime'), detail: t('providers.overview.needsRuntimeLead'), next: 'runtime' };
      const detail = provider.status_detail?.trim() ?? '';
      return { tone: 'warning', text: t('providers.phrase.needsSetup'), detail: detail.length === 0 ? t('providers.overview.checkSetupLead') : detail, next: 'settings' };
    }
    case 'ready': {
      if (provider.runtime?.update_required === true) return { tone: 'warning', text: t('providers.phrase.updateRequired'), next: null };
      const until = planLimitUntil(provider, now);
      if (until !== null) return { tone: 'warning', text: t('providers.phrase.limitUntil', { time: formatTimeOfDay(until, context, now) }), next: null };
      const days = daysUntilLapse(provider, now);
      if (days !== null) return { tone: 'warning', text: t('providers.phrase.lapses', { count: days }), next: provider.kind === 'api' ? 'key' : 'signIn' };
      if ((provider.runtime?.updates.length ?? 0) > 0) return { tone: 'info', text: t('providers.phrase.updateAvailable'), next: null };
      if (provider.runtime?.state === 'stopped') return { tone: 'idle', text: t('providers.phrase.stopped'), next: null };
      return { tone: 'success', text: t('providers.phrase.ready'), next: null };
    }
  }
}

/** What a provider is, in a few words: "Subscription · Pro", "API key · 2 endpoints". */
export function providerWhat(provider: Provider | ProviderListRow, harnessTitle: (kind: string) => string, t: TFunction): string {
  const credential = provider.kind === 'subscription' ? t('providers.what.subscription', { harness:harnessTitle(provider.harness_kind ?? '') }) : t(('upstreams' in provider ? endpointsNeedNoKey(provider) : provider.no_key_needed) ? 'providers.what.apiOpen' : 'providers.what.api', { count:'upstreams' in provider ? provider.upstreams.length : provider.upstreams_count });
  const plan = provider.facts.plan;
  if (detailState(provider,'plan') !== 'told' || plan === null) return credential;
  const title = 'sources' in provider ? factsSource(provider).descriptor?.facts?.plans[plan] ?? plan : provider.plan_label ?? plan;
  return t('providerDetails.whatWithPlan',{ credential,plan:title });
}

/** Runtime storage words stay internal; a newer state's identifier is never printed to a person. */
export function providerRuntimeState(state:string,t:TFunction):string {
  switch(state) {
    case 'creating': case 'starting':return t('providerRecovery.runtimeCreating');
    case 'running':return t('providerRecovery.runtimeRunning');
    case 'stopping':return t('providerRecovery.runtimeStopping');
    case 'stopped':return t('providerRecovery.runtimeStopped');
    case 'failed':return t('providerRecovery.runtimeFailed');
    case 'retired':return t('providerRecovery.runtimeRetired');
    case 'removed':return t('providerRecovery.runtimeRemoved');
    default:return t('providerRecovery.runtimeUnknown');
  }
}

/** The model picker and start review use the setup cause the binding actually observed. */
export function providerSetupWords(setup: BindingProviderSetup | null | undefined, t: TFunction): string {
  switch (setup?.reason) {
    case 'sign_in_lapsed': return t('providers.phrase.signInLapsed');
    case 'needs_sign_in': return t('providers.phrase.needsSignIn');
    case 'needs_model': return t('providers.phrase.needsModel');
    default: return t('providers.phrase.needsSetup');
  }
}
