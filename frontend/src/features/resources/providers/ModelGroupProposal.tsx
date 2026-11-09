/** A reported family fills the existing model editor only after a manager chooses to review it. */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { ModelSuggestion, Provider, ProviderModel } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { adminSpec } from '@/ui/AdminAct';
import { ModelEditorDialog } from './ModelEditor';
import { proposedModelGroup } from './modelOrigins';

export function ModelGroupProposal({ provider, model, observed, manage }: { provider: Provider; model: ProviderModel; observed: ModelSuggestion | undefined; manage: boolean }) {
  const { t } = useTranslation();
  const [reviewing, setReviewing] = useState<string | null>(null);
  if (!manage) return null;
  const group = proposedModelGroup(observed);
  return <>
    {group !== null && !model.groups.includes(group) && <Button admin={adminSpec(provider, 'provider.manage')} size="sm" variant="ghost" className="self-start" onClick={() => setReviewing(group)}>{t('providerFill.groupProposal', { name: group })}</Button>}
    {reviewing !== null && <ModelEditorDialog provider={provider} model={model} proposedGroup={reviewing} onClose={() => setReviewing(null)} />}
  </>;
}
