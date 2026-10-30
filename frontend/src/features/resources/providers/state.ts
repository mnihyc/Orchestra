/** What a provider is and how it stands, as one phrase each: "Subscription · Pro", "API key · 2 endpoints", "Needs sign-in", "Limit reached · resets 14:00", with what to do about it.
 */
import type { TFunction } from 'i18next';

import type { Provider } from '@/generated/api-types';
import { DAY_MS, formatTimeOfDay, type TimeContext } from '@/lib/time';
import type { DotTone } from '@/ui/StatusDot';
import { detailState, endpointsNeedNoKey, factsSource } from './details';

/** As shipped, how many days before a token lapses its provider says so (`providers.expiry_notice_days`). */
export const EXPIRY_NOTICE_DAYS = 14;


/** What a provider's page can point to as the next step of setting it up. */
export type NextStep = 'key' | 'models' | 'signIn' | 'runtime' | 'settings' | null;

/** The same next step is offered by the overview, its heading and the provider's list row. */
export const PROVIDER_NEXT = {
  key: { tab: 'key', label: 'resources.act.nextKey' },
  models: { tab: 'models', label: 'resources.act.nextModels' },
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
export const hasEnabledModel = (provider: Pick<Provider, 'models'>): boolean => provider.models.some((model) => model.enabled);

/** The days left before the provider's token lapses, when it has one that lapses within the notice. */
export function daysUntilLapse(provider: Pick<Provider, 'facts'>, now: number): number | null {
  const ends = provider.facts.credential?.expires_at;
  if (ends === null || ends === undefined) return null;
  const left = Math.ceil((Date.parse(ends) - now) / DAY_MS);
  return left <= EXPIRY_NOTICE_DAYS ? Math.max(left, 0) : null;
}

export function providerState(provider: Provider, t: TFunction, context: TimeContext, now: number): ProviderState {
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
      const credential = provider.facts.credential;
      const signedIn = credential?.yields === 'login' || credential?.yields === 'token';
      const ends = credential?.expires_at;
      const expired = ends !== null && ends !== undefined && Date.parse(ends) <= now;
      if (provider.kind === 'subscription' && (!signedIn || expired)) return { tone: 'warning', text: t('providers.phrase.needsSignIn'), detail: t('providers.overview.needsSignInLead'), next: 'signIn' };
      if (provider.kind === 'api' && !endpointsNeedNoKey(provider) && !provider.key_set && provider.key_machine === null) return { tone: 'warning', text: t('providers.phrase.needsKey'), detail: t('providers.overview.needsKeyLead'), next: 'key' };
      if (!hasEnabledModel(provider)) return { tone: 'warning', text: t('providers.phrase.needsModel'), detail: t('providers.overview.needsModelLead'), next: 'models' };
      if (provider.placement === 'managed' && (provider.runtime?.state === 'stopped' || provider.runtime === null && provider.machine === null)) {
        return { tone: 'warning', text: t('providers.phrase.needsRuntime'), detail: provider.status_detail ?? t('providers.overview.needsRuntimeLead'), next: 'runtime' };
      }
      const detail = provider.status_detail?.trim() ?? '';
      return { tone: 'warning', text: t('providers.phrase.needsSetup'), detail: detail.length === 0 ? t('providers.overview.checkSetupLead') : detail, next: 'settings' };
    }
    case 'ready': {
      if (provider.runtime?.update_required === true) return { tone: 'warning', text: t('providers.phrase.updateRequired'), next: null };
      const days = daysUntilLapse(provider, now);
      if (days !== null) return { tone: 'warning', text: t('providers.phrase.lapses', { count: days }), next: provider.kind === 'api' ? 'key' : 'signIn' };
      if ((provider.runtime?.updates.length ?? 0) > 0) return { tone: 'info', text: t('providers.phrase.updateAvailable'), next: null };
      if (provider.runtime?.state === 'stopped') return { tone: 'idle', text: t('providers.phrase.stopped'), next: null };
      return { tone: 'success', text: t('providers.phrase.ready'), next: null };
    }
  }
}

/** What a provider is, in a few words: "Subscription · Pro", "API key · 2 endpoints". */
export function providerWhat(provider: Provider, harnessTitle: (kind: string) => string, t: TFunction): string {
  const credential = provider.kind === 'subscription' ? t('providers.what.subscription', { harness:harnessTitle(provider.harness_kind ?? '') }) : t(endpointsNeedNoKey(provider) ? 'providers.what.apiOpen' : 'providers.what.api', { count:provider.upstreams.length });
  const plan = provider.facts.plan;
  if (detailState(provider,'plan') !== 'told' || plan === null) return credential;
  const title = factsSource(provider).descriptor?.facts?.plans[plan] ?? plan;
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
