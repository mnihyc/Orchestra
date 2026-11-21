/** The same model/effort control as threads, bound to the root's configured native Ask entries. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { askModelsQuery } from '@/api/ask';
import { ModelEffort } from '@/features/composer/ModelEffort';
import { RequestOptions } from '@/features/models/RequestOptions';
import { useViewShown } from '@/features/workbench/viewOpening';
import type { AskModel } from '@/generated/api-types';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { askModelExclusionText, askSelection, configuredAskEntries } from './modelChoices';
import { backendIdentity } from '@/features/settings/capabilityBackendValue';
import { ResourceDetails } from '@/features/resources/DetailSections';
import { CapabilityPlan } from '@/features/timeline/tools/CapabilityPlan';

export function AskModelChoice({ root, value, disabled, onChange }: { root: string; value: AskModel | null; disabled: boolean; onChange: (value: AskModel | null) => void }) {
  const { t } = useTranslation();
  const shown = useViewShown();
  const [open, setOpen] = useState(false);
  const options = useQuery({ ...askModelsQuery(root), enabled: shown && root !== '' });
  const selected = value ?? options.data?.default_model;
  const active = selected === null || selected === undefined ? undefined : options.data?.models.find((row) => backendIdentity(row.selection.backend) === backendIdentity(selected.backend));
  const subscription = active?.provider?.kind === 'subscription' ? active.provider : null;
  const entries = configuredAskEntries(options.data, value);
  const state = options.isLoading ? <SkeletonRows rows={3} /> : options.isError ? <ErrorState error={options.error} onRetry={() => void options.refetch()} />
    : entries.length === 0 ? <StateLine title={t('ask.noModels')} /> : undefined;
  return <><ModelEffort open={open && !disabled} onOpenChange={setOpen} choice={{ value: askSelection(value), entries, disabled, label: t('ask.model'), onChange: (next) => {
    if (next === null) onChange(null);
    else if (next.backend !== undefined) onChange({ backend: next.backend, params: next.params });
  },
    defaultChoice: { label: t('ask.defaultModel'), value: askSelection(options.data?.default_model) }, state,
    renderParameters: (entry, selected, change, readOnly) => {
      const schema = options.data?.models.find((configured) => entry.value.backend !== undefined && backendIdentity(configured.selection.backend) === backendIdentity(entry.value.backend))?.request_option_schema;
      // A model that offers no request option gets no row for them: an empty box said nothing.
      const offered = schema !== null && typeof schema === 'object' && 'properties' in schema && schema.properties !== null && typeof schema.properties === 'object' ? Object.keys(schema.properties).length : 0;
      return schema === null || schema === undefined || offered === 0 ? null : <RequestOptions schema={schema} value={selected.params ?? {}} disabled={readOnly} onChange={change} managedEffort hideHeading />;
    },
  }} />
    {value !== null && options.isSuccess && active?.allowed !== true &&
      <span className="text-xs text-text-muted">{active?.unavailable_reason ? askModelExclusionText(active.unavailable_reason, t) : t('ask.modelUnavailable')}</span>}
    {subscription !== null && <ResourceDetails label={t('timeline.capabilitySubscription')}>
      {() => <CapabilityPlan providerId={subscription.id} />}
    </ResourceDetails>}
  </>;
}
