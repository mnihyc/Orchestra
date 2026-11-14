/** Request option forms consume the server's resolved schema, just as dispatch does. */
import { useQuery } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { providerModelsQuery, providerQuery } from '@/api/resources';
import { record } from '@/features/resources/UnitCommands';
import { OtherJson, SchemaInput, SchemaManagedPaths } from '@/features/resources/templates/SchemaForm';
import type { Schema } from '@/features/resources/templates/schema';
import { replaceAt, valueAt } from '@/features/resources/templates/schema';
import { ErrorAlert } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { Disclosure } from '@/ui/Disclosure';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';

export interface RequestOptionsProps {
  label?: string;
  schema?: unknown;
  value: unknown;
  disabled?: boolean;
  onChange: (value: unknown) => void;
  /** The surrounding ModelEffort picker owns effort, so it has one control and one value. */
  managedEffort?: boolean;
  /** The surrounding part already shows the title (a row that opens these options). */
  hideHeading?: boolean;
}

export function RequestOptionsDisclosure({ label, value, reveal, children }: { label: string; value: unknown; reveal?: boolean; children: ReactNode }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  return <Disclosure title={label} lead={t('providerOptions.fieldsSet', { figure: formatNumber(Object.keys(record(value)).length, locale) })} reveal={reveal}>
    <div className="flex min-w-0 flex-col gap-2 py-3">{children}</div>
  </Disclosure>;
}

const EFFORT_PATHS = [['effort'], ['reasoning_effort'], ['reasoning', 'effort'], ['output_config', 'effort'], ['generation_config', 'thinking_level']] as const;
function omitPath(value: unknown, path: readonly string[]): Record<string, unknown> {
  const current = record(value), [name, ...rest] = path;
  if (valueAt(value, path) === undefined) return current;
  if (name === undefined || !Object.hasOwn(current, name)) return current;
  const next = { ...current };
  const { [name]: _removed, ...remaining } = next;
  if (rest.length === 0) return remaining;
  const child = omitPath(next[name], rest);
  return Object.keys(child).length === 0 ? remaining : { ...next, [name]: child };
}
export function managedEffortForm(schema: unknown, value: unknown) {
  return {
    schema,
    value: EFFORT_PATHS.reduce<unknown>((current, path) => omitPath(current, path), value),
    restore: (edited: unknown) => EFFORT_PATHS.reduce((current, path) => {
      const original = valueAt(value, path);
      return original === undefined ? current : replaceAt(current, path, original);
    }, record(EFFORT_PATHS.reduce<unknown>((current, path) => omitPath(current, path), edited))),
  };
}

export function RequestOptions({ label, schema, value, disabled = false, onChange, managedEffort = false, hideHeading = false }: RequestOptionsProps) {
  const { t } = useTranslation();
  const form = managedEffort ? managedEffortForm(schema, value) : { schema, value, restore: (next: unknown) => next };
  const change = (next: unknown) => onChange(form.restore(next ?? {}));
  const root: Schema = { ...record(form.schema), title: label ?? t('providerOptions.title') };
  const fields = form.schema === undefined ? <OtherJson label={label ?? t('providerOptions.title')} objectOnly value={form.value} disabled={disabled} onChange={change} />
    : <SchemaManagedPaths.Provider value={managedEffort ? EFFORT_PATHS : []}><SchemaInput root={root} schema={root} value={form.value} parts={[]} disabled={disabled} problems={[]} driver="provider-options" hideHeading onChange={change} /></SchemaManagedPaths.Provider>;
  return hideHeading ? fields : <RequestOptionsDisclosure label={label ?? t('providerOptions.title')} value={form.value}>{fields}</RequestOptionsDisclosure>;
}

/** Configuration editors fetch only the selected provider/model; no trial requests discover options. */
export function ModelRequestOptions({ providerId, model, protocol, ...props }: Omit<RequestOptionsProps, 'schema'> & { providerId: string; model: string; protocol?: string | null }) {
  const models = useQuery({ ...providerModelsQuery(providerId), enabled: providerId !== '' && model !== '', retry: false });
  const provider = useQuery({ ...providerQuery(providerId), enabled: providerId !== '' && model !== '', retry: false });
  const selected = models.data?.find((entry) => entry.model_name === model);
  const schemas = record(selected?.option_constraints);
  const format = protocol === null ? undefined : protocol ?? provider.data?.protocols[0];
  const schema = format === undefined ? undefined : schemas[format];
  const failure = models.error ?? provider.error;
  if (models.isLoading || provider.isLoading) return <SkeletonRows reads={[models, provider]} rows={2} />;
  return <div className="flex min-w-0 flex-col gap-2"><RequestOptions {...props} schema={schema} />{failure !== null && <ErrorAlert error={failure} />}</div>;
}
