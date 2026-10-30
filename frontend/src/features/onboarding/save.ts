/** First-steps choices saved through the canonical atomic preference request. */
import { setFirstSteps } from '@/api/firstSteps';
import type { FirstStepsChoice } from '@/features/onboarding/state';

/** The server merges current help and navigation; success alone changes what is shown. */
export function saveFirstSteps(choice: FirstStepsChoice): Promise<Record<string, unknown>> {
  return setFirstSteps(choice);
}
