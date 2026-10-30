/** Routing values stay in requests; every wizard surface uses the reported display name. */
import type { TFunction } from 'i18next';

import type { ProviderModel } from '@/generated/api-types';
import { offeredChoice } from '@/i18n/choices';
import { modelDisplayName } from '@/features/models/names';

export function modelLabel(routing: string | null, models: readonly Pick<ProviderModel, 'model_name' | 'display_name'>[], currentDisplay: string | null, t: TFunction): string {
  const reported = models.find((model) => model.model_name === routing)?.display_name.trim();
  if (reported !== undefined && reported !== '') return modelDisplayName(reported, t);
  if (currentDisplay !== null && currentDisplay.trim() !== '' && currentDisplay !== routing) return modelDisplayName(currentDisplay, t);
  return t(routing === null ? 'changeCompact.noModel' : 'changeCompact.unnamedModel');
}

export function effortLabel(effort: string | null, t: TFunction): string | null {
  return effort === null ? null : offeredChoice(t, 'effort', effort)?.name ?? t('changeCompact.otherEffort');
}
