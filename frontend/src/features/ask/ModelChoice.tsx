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
import { configuredAskEntries } from './modelChoices';

export function AskModelChoice({ root, value, disabled, onChange }: { root: string; value: AskModel | null; disabled: boolean; onChange: (value: AskModel | null) => void }) {
  const { t } = useTranslation();
  const shown = useViewShown();
  const [open, setOpen] = useState(false);
  const options = useQuery({ ...askModelsQuery(root), enabled: shown && root !== '' });
  const entries = configuredAskEntries(options.data);
  const state = options.isLoading ? <SkeletonRows rows={3} /> : options.isError ? <ErrorState error={options.error} onRetry={() => void options.refetch()} />
    : entries.length === 0 ? <StateLine title={t('ask.noModels')} /> : undefined;
  return <ModelEffort open={open && !disabled} onOpenChange={setOpen} choice={{ value, entries, disabled, label: t('ask.model'), onChange: (next) => onChange(next),
    defaultChoice: { label: t('ask.defaultModel'), value: options.data?.default_model ?? null }, state,
    renderParameters: (entry, selected, change, readOnly) => {
      const schema = options.data?.models.find((configured) => configured.selection.provider_id === entry.value.provider_id && configured.selection.model === entry.value.model)?.request_option_schema;
      // A model that offers no request option gets no row for them: an empty box said nothing.
      const offered = schema !== null && typeof schema === 'object' && 'properties' in schema && schema.properties !== null && typeof schema.properties === 'object' ? Object.keys(schema.properties).length : 0;
      return schema === null || schema === undefined || offered === 0 ? null : <RequestOptions schema={schema} value={selected.params ?? {}} disabled={readOnly} onChange={change} managedEffort hideHeading />;
    },
  }} />;
}
