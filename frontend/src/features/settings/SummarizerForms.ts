/** Registered producers describe their configuration; the setting row remains the writer. */
import { isRecord } from './draftChecks';

export interface SummarizerForm { name: string; title: string; config_schema: unknown; fields: string[] }

/** The kinds actually installed, with their declared forms and instruction fields. */
export function summarizerForms(schema: unknown): SummarizerForm[] {
  const offered = isRecord(schema) ? schema['x-summarizers'] : undefined;
  return Array.isArray(offered) ? offered.flatMap((item) => {
    if (!isRecord(item) || typeof item.name !== 'string' || item.name === '' || typeof item.title !== 'string') return [];
    return [{ name: item.name, title: item.title, config_schema: item.config_schema,
      fields: Array.isArray(item.fields) ? item.fields.filter((field): field is string => typeof field === 'string') : [] }];
  }) : [];
}

function properties(form: SummarizerForm | undefined): Record<string, unknown> {
  const schema = form?.config_schema;
  return isRecord(schema) && isRecord(schema.properties) ? schema.properties : {};
}

/** Initial values supplied by a registered configuration, including required fields with defaults. */
export function summarizerDefaults(form: SummarizerForm | undefined): Record<string, unknown> {
  return Object.fromEntries(Object.entries(properties(form)).flatMap(([name, field]) =>
    isRecord(field) && Object.hasOwn(field, 'default') ? [[name, field.default]] : []));
}

/** A model count's declared default; an undeclared default uses the built-in count. */
export function summarizerCount(form: SummarizerForm | undefined, member: string): number | undefined {
  const field = properties(form)[member];
  const value = isRecord(field) ? field.default : undefined;
  return typeof value === 'number' && Number.isFinite(value) && value >= 1 ? value : undefined;
}
