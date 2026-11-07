/** Request option forms consume the server's resolved schema, just as dispatch does. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { providerModelsQuery, providerQuery } from '@/api/resources';
import { record } from '@/features/resources/UnitCommands';
import { OtherJson, SchemaInput } from '@/features/resources/templates/SchemaForm';
import type { Schema } from '@/features/resources/templates/schema';
import { ErrorAlert } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export interface RequestOptionsProps {
  label?: string;
  schema?: unknown;
  value: unknown;
  disabled?: boolean;
  onChange: (value: unknown) => void;
}

export function RequestOptions({ label, schema, value, disabled = false, onChange }: RequestOptionsProps) {
  const { t } = useTranslation();
  if (schema === undefined) return <OtherJson label={label ?? t('providerOptions.title')} objectOnly value={value} disabled={disabled} onChange={onChange} />;
  const root: Schema = { ...record(schema), title: label ?? t('providerOptions.title') };
  return <SchemaInput root={root} schema={root} value={value} parts={[]} disabled={disabled} problems={[]} driver="provider-options" onChange={(next) => onChange(next ?? {})} />;
}

/** Configuration editors fetch only the selected provider/model; no trial requests discover options. */
export function ModelRequestOptions({ providerId, model, protocol, ...props }: Omit<RequestOptionsProps, 'schema'> & { providerId: string; model: string; protocol?: string }) {
  const models = useQuery({ ...providerModelsQuery(providerId), enabled: providerId !== '' && model !== '', retry: false });
  const provider = useQuery({ ...providerQuery(providerId), enabled: providerId !== '' && model !== '', retry: false });
  const selected = models.data?.find((entry) => entry.model_name === model);
  const schemas = record(selected?.option_constraints);
  const format = protocol ?? provider.data?.protocols[0];
  const schema = format === undefined ? undefined : schemas[format];
  const failure = models.error ?? provider.error;
  if (models.isLoading || provider.isLoading) return <SkeletonRows rows={2} />;
  return <div className="flex min-w-0 flex-col gap-2"><RequestOptions {...props} schema={schema} />{failure !== null && <ErrorAlert error={failure} />}</div>;
}
