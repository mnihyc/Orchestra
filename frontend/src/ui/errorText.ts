/** An error in words: what happened and what to do, in the reader's language, with what an administrator needs to find it. */
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';

import { ApiError, ReceiptAcknowledged, databaseWaitOf, invitationIssuerAuthorityLost } from '@/api/client';
import type { TranslationKey } from '@/i18n';
import { apiErrorSentence, hasErrorSentence } from '@/i18n/errors';
import { useAppearance } from '@/lib/appearance';
import { NetworkError, networkFailureDetails } from '@/lib/requests';
import { formatSpan, formatTimeOfDay } from '@/lib/time';
import { formatBytes, formatNumber } from '@/lib/format';
import nativeAliases from '@/i18n/error-aliases.json';

export interface ErrorText {
  /** What happened and what to do, as one or two sentences. */
  message: string;
  /** The native source’s own safe words, shown below the localized explanation. */
  diagnostic?: string;
  /** The stable code of the error, for the "Details": the API's own, or a name for what the page met. */
  code: string;
  /** The id of the request, by which the server's logs find it. */
  requestId: string | null;
  /** The HTTP status, where something answered. */
  status: number | null;
  /** Whether asking again could help. */
  retryable: boolean;
  /** A successful write with a bounded receipt is not a failure or permission to resend. */
  acknowledged?: true;
}

const FAILURE_WORDS: Readonly<Record<string, string | undefined>> = nativeAliases;

function sentence(code: string, t: TFunction, locale: string): string | null {
  const alias = Object.hasOwn(FAILURE_WORDS, code) ? FAILURE_WORDS[code] : undefined;
  if (alias !== undefined && hasErrorSentence(alias,locale)) return t(alias as TranslationKey);
  const key = `errors.${code}`;
  return hasErrorSentence(key,locale) ? t(key as TranslationKey) : null;
}

/** The sentence of a reason an answer names by a word (`reason`), with the work (`work`) and the step (`step`) it names; `null` when the catalogue has no sentence for the word. */
function reasonSentence(error: ApiError, t: TFunction, locale: string): string | null {
  const { reason, work, step } = error.problem;
  if (reason === 'machine_upstream' && error.problem.policy === 'provider.upstreams') {
    const entries = error.problem.allowed_upstreams;
    if (Array.isArray(entries)) {
      const addresses = entries.filter((entry):entry is string => typeof entry === 'string');
      return t('providerCreation.upstreamsListed', { policy:'provider.upstreams', addresses:addresses.length===0?t('providerCreation.upstreamsNone'):addresses.join(', ') });
    }
    return t('providerCreation.upstreamsKept', { policy:'provider.upstreams' });
  }
  if (reason === 'harness_error' && typeof error.problem.policy === 'string'
    && ['agent.programs', 'tools.fetch', 'agent.harnesses'].includes(error.problem.policy)) {
    return t('ui.error.harnessBuildPolicy', { key: error.problem.policy });
  }
  if (typeof reason !== 'string' || !hasErrorSentence(`errors.reason.${reason}`, locale)) return null;
  const doing = typeof work === 'string' && hasErrorSentence(`errors.reasonWork.${work}`, locale) ? t(`errors.reasonWork.${work}` as TranslationKey) : '';
  const phases = work === 'remove' ? 'providers.runtime.removalPhase' : 'providers.runtime.phase';
  const where = typeof step !== 'string' ? '' : hasErrorSentence(`${phases}.${step}`, locale) ? t(`${phases}.${step}` as TranslationKey) : step;
  return t(`errors.reason.${reason}` as TranslationKey, { work: doing, step: where });
}

/** What a refused Ask says: the API names the reason it refused for, and the catalogue gives its sentence in the reader's language. */
function refusalSentence(error: ApiError, t: TFunction, locale: string): string | null {
  const reason = error.problem.reason;
  if (error.status !== 403 || typeof reason !== 'string') return null;
  const key = `errors.ask.${reason}`;
  return hasErrorSentence(key, locale) ? t(key as TranslationKey) : null;
}

/** Captured upstream rules use the same failure line, with the actual amount rather than a UI default. */
function upstreamSentence(details: unknown, t: TFunction, context: {locale:string;timeZone?:string|undefined}): string | null {
  if (typeof details !== 'object' || details === null || !('upstream' in details)) return null;
  const fact = details.upstream;
  if (typeof fact !== 'object' || fact === null || !('rule' in fact)) return null;
  if (fact.rule === 'whole-reply-size' && 'bytes' in fact && typeof fact.bytes === 'number' && Number.isFinite(fact.bytes) && fact.bytes > 0) return t('ui.error.upstreamSize', {size:formatBytes(fact.bytes,context.locale)});
  if (!('seconds' in fact) || typeof fact.seconds !== 'number' || !Number.isFinite(fact.seconds) || fact.seconds <= 0) return null;
  const time = formatSpan(fact.seconds * 1000,context);
  switch (fact.rule) {
    case 'response-header': return t('ui.error.upstreamHeader',{time});
    case 'whole-reply': return t('ui.error.upstreamWhole',{time});
    case 'silence': return t('ui.error.upstreamSilence',{time});
    case 'connection': return t('ui.error.upstreamConnection',{time});
    default: return null;
  }
}

/** Puts an error into words. `t` is the catalogue; the time of a "try again at" is said in `context`. */
export function errorText(
  error: unknown,
  t: TFunction,
  context: { locale: string; timeZone?: string | undefined },
): ErrorText {
  if (error instanceof ReceiptAcknowledged) return {
    message: t('ui.receipt.accepted'), code: 'receipt.acknowledged', requestId: error.requestId === '' ? null : error.requestId,
    status: error.status, retryable: false, acknowledged: true,
  };
  if (error instanceof ApiError) {
    const at = error.retryAt === null ? null : formatTimeOfDay(error.retryAt, context);
    const wait = databaseWaitOf(error);
    const databaseMessages = { acquire: 'databaseWait.messages.acquire', statement: 'databaseWait.messages.statement', idle_transaction: 'databaseWait.messages.idleTransaction' } as const;
    const message = (wait === null ? null : t(databaseMessages[wait.phase])) ?? upstreamSentence(error.problem,t,context) ?? refusalSentence(error, t, context.locale) ?? (invitationIssuerAuthorityLost(error) ? t('auth.invite.issuerAuthorityLost')
      : error.code === 'rate_limited' && at !== null ? t('errorsAt.rate_limited', { time: at }) : apiErrorSentence(error.code, error.status, t, context.locale));
    const reason = reasonSentence(error, t, context.locale);
    return {
      message,
      ...(reason === null ? {} : { diagnostic: reason }),
      code: error.code,
      requestId: error.requestId === '' ? null : error.requestId,
      status: error.status,
      // An answer of the API is an answer; only "not now" is worth asking again.
      retryable: error.status === 429 || error.status >= 500,
    };
  }
  if (error instanceof NetworkError) {
    const details = networkFailureDetails(error);
    let message = t(`network.${error.failure}`);
    if (details !== null && error.failure === 'silent') {
      const duration = details.limitMs < 1_000 ? formatNumber(details.limitMs, context.locale, { style: 'unit', unit: 'millisecond', unitDisplay: 'narrow' }) : formatSpan(details.limitMs, context);
      message = t(details.responseStarted ? 'ui.error.silenceBody' : 'ui.error.silenceResponse', { duration });
    } else if (details !== null && error.failure === 'failed') {
      message = t(details.responseStarted ? 'ui.error.networkBody' : 'ui.error.networkResponse');
    }
    return {
      message,
      code: `network.${error.failure}`,
      requestId: null,
      status: error.status,
      retryable: error.transient,
    };
  }
  if (typeof error === 'object' && error !== null && 'code' in error && typeof error.code === 'string') {
    if (error.code === 'too_many_subscriptions') return {
      message: t('ui.error.subscriptionCapacity'), code: error.code, requestId: null, status: null, retryable: true,
    };
    const nativeDetails = 'details' in error && typeof error.details === 'object' && error.details !== null ? error.details : null;
    const upstream = upstreamSentence(nativeDetails,t,context);
    if (upstream !== null) return {message:upstream,code:error.code,requestId:null,status:null,retryable:error.code==='timeout'};
    const reason = 'reason' in error && typeof error.reason === 'string' ? error.reason
      : nativeDetails !== null && 'reason' in nativeDetails && typeof nativeDetails.reason === 'string' ? nativeDetails.reason : null;
    if (error.code === 'model_unavailable' && reason === 'credit_program') {
      return { message: t('ui.error.creditProgramRestricted'), code: error.code, requestId: null, status: null, retryable: false };
    }
    if(error.code==='model_rerouted' && 'details' in error && typeof error.details==='object' && error.details!==null && 'from_model' in error.details && 'to_model' in error.details && typeof error.details.from_model==='string' && typeof error.details.to_model==='string' && error.details.from_model.trim()!=='' && error.details.to_model.trim()!=='') {
      const fromModel=error.details.from_model,toModel=error.details.to_model,details=error.details;
      const from='from_display' in details && typeof details.from_display==='string' ? details.from_display : fromModel;
      const to='to_display' in details && typeof details.to_display==='string' ? details.to_display : toModel;
      return {message:t('ui.error.modelReroutedNamed',{from,to}),code:error.code,requestId:null,status:null,retryable:false};
    }
    if(error.code==='mcp_server_failed' && 'details' in error && typeof error.details==='object' && error.details!==null && 'server' in error.details && typeof error.details.server==='string') {
      const diagnostic='message' in error.details && typeof error.details.message==='string' && error.details.message!=='' ? error.details.message : undefined;
      return {message:t('ui.error.mcpServerFailedNamed',{server:error.details.server}),...(diagnostic===undefined?{}:{diagnostic}),code:error.code,requestId:null,status:null,retryable:false};
    }
    const mapped = sentence(error.code, t, context.locale);
    // Native programs and jobs retain their diagnostics. A newer code still gets translated
    // recovery copy rather than an unreviewed program sentence in the wrong language.
    return { message: mapped ?? t('ui.error.operationFailed'), code: error.code, requestId: null, status: null,
      retryable: /timeout|network|offline|rate|overload/u.test(error.code) };
  }
  if (typeof error === 'string') return { message: sentence(error.trim(), t, context.locale) ?? t('ui.error.operationFailed'), code: 'operation', requestId: null, status: null, retryable: false };
  return { message: t('errors.page'), code: 'page', requestId: null, status: null, retryable: true };
}

/** `errorText` with the catalogue and the reader's time zone at hand. */
export function useErrorText(): (error: unknown) => ErrorText {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  return (error) => errorText(error, t, { locale, timeZone });
}
