import type { TFunction } from 'i18next';

import { ApiError } from '@/api/client';

/** Whether an answer refused an act on a provider's runtime for a reason that doing it by force changes. */
export function canForce(error: unknown): boolean {
  return error instanceof ApiError && error.is('provider.unavailable') && error.problem.can_force === true;
}

/** The step the work on a runtime has come to, in words: taking a runtime away has steps of its own. */
export function stepWords(removal: boolean, phase: string, t: TFunction): string {
  return t(`providers.runtime.${removal ? 'removalPhase' : 'phase'}.${phase}`, { defaultValue: phase });
}
