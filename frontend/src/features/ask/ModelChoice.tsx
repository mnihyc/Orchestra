/** The same model/effort control as threads, bound to the root's configured native Ask entries. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { askModelsQuery } from '@/api/ask';
import { ModelEffort } from '@/features/composer/ModelEffort';
import { RequestOptions } from '@/features/models/RequestOptions';
import { useViewShown } from '@/features/workbench/viewOpening';
import type { AskPreference } from '@/generated/api-types';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { askModelExclusionText, askSelection, askTargetIdentity, configuredAskEntries } from './modelChoices';
import { backendIdentity } from '@/features/settings/capabilityBackendValue';
import { ResourceDetails } from '@/features/resources/DetailSections';
import { CapabilityPlan } from '@/features/timeline/tools/CapabilityPlan';

export function AskModelChoice({ root, value, disabled, onChange }: { root: string; value: AskPreference | null; disabled: boolean; onChange: (value: AskPreference | null) => void }) {
  const { t } = useTranslation();
  const shown = useViewShown();
  const [open, setOpen] = useState(false);
  const options = useQuery({ ...askModelsQuery(root), enabled: shown && root !== '' });
  const selected = value ?? options.data?.default_model;
  const active = selected === null || selected === undefined ? undefined : options.data?.models.find((row) => askTargetIdentity(row.selection) === askTargetIdentity(selected));
  const subscription = active?.provider?.kind === 'subscription' ? active.provider : null;
  const entries = configuredAskEntries(options.data, value);
  const poolId = value && 'target' in value ? askTargetIdentity(value) : undefined;
  const defaultTarget = options.data?.default_model;
  const poolRows = (options.data?.models ?? []).flatMap((row) => {
    if (!('target' in row.selection) || !row.pool_name) return [];
    const id = askTargetIdentity(row.selection);
    return [{ id, name: row.selection.model ? [row.pool_name, row.selection.model].join(t('startFlow.separator')) : row.pool_name, checked: id === poolId, disabled: disabled || !row.allowed,
      detail: row.unavailable_reason && askModelExclusionText(row.unavailable_reason, t) }];
  });
  const state = options.isLoading ? <SkeletonRows rows={3} /> : options.isError ? <ErrorState error={options.error} onRetry={() => void options.refetch()} />
    : entries.length === 0 && poolRows.length === 0 ? <StateLine title={t('ask.noModels')} /> : undefined;
  return <><ModelEffort open={open && !disabled} onOpenChange={setOpen} choice={{ value: askSelection(value), entries, disabled, pools: {
    rows: poolRows, selectedId: poolId, selectedName: poolId ? poolRows.find((row) => row.id === poolId)?.name ?? t('ask.modelUnavailable') : undefined,
    defaultName: defaultTarget && 'target' in defaultTarget ? poolRows.find((row) => row.id === askTargetIdentity(defaultTarget))?.name : undefined,
    onChoose: (id) => {
      const row = options.data?.models.find((entry) => entry.allowed && askTargetIdentity(entry.selection) === id);
      if (row && 'target' in row.selection) onChange(row.selection);
    },
  }, label: t('ask.model'), onChange: (next) => {
    if (next === null) onChange(null);
    else if (next.backend !== undefined) onChange({ backend: next.backend, params: next.params });
  },
    defaultChoice: { label: t('ask.defaultModel'), value: askSelection(options.data?.default_model) }, state,
    renderParameters: (entry, selected, change, readOnly) => {
      const schema = options.data?.models.find((configured) => entry.value.backend !== undefined && 'backend' in configured.selection && backendIdentity(configured.selection.backend) === backendIdentity(entry.value.backend))?.request_option_schema;
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
