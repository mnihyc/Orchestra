/** The contract's outcome filters, used by addresses, selectors and request rows. */
import type { UsageOutcomeFilter } from '@/generated/api-types';

export const OUTCOMES = ['running','errors','not_measured','done','provider_exhausted','provider_unauthorized','provider_busy','provider_unreachable','model_unavailable','model_refused','context_overflow','environment_unreachable','harness_error','session_lost','forced','start_timeout','cap_reached'] as const satisfies readonly UsageOutcomeFilter[];

export function readOutcome(value: unknown): UsageOutcomeFilter | undefined {
  return OUTCOMES.find((outcome) => outcome === value);
}

/** Keep tool, session and harness failures visible in Requests without blaming the provider. */
export const PROVIDER_ERRORS: readonly string[] = ['provider_exhausted','provider_unauthorized','provider_busy','provider_unreachable','model_unavailable','model_refused'];
export const providerError = (code: string): boolean => PROVIDER_ERRORS.includes(code);
