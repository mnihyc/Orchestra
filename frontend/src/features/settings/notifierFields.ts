/** What a kind of channel asks for, turned into what is sent and back: which of its fields are secrets, the values of the others as a config, and what a form starts with.
 *
 * A notifier (the mail server, the bot, the webhook, anything installed) declares its fields. The
 * ones that are secrets are written and never read: they go in `secrets`, only when the person typed
 * something, and an edit that leaves one empty keeps the secret as it is. The others make up the
 * `config`, which is checked by the server against the same declaration.
 */
import type { Field } from '@/generated/api-types';
import type { FormValue, FormValues } from '@/ui/FieldForm';

const SECRET_TYPES: ReadonlySet<Field['type']> = new Set(['secret', 'secret_text', 'secret_map']);

/** Whether a field is written and never read. */
export const isSecret = (field: Pick<Field, 'type'>): boolean => SECRET_TYPES.has(field.type);

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** The values a form starts with: what the field declares for a new one, what the config holds for one that exists. Secrets always start empty. */
export function startingValues(fields: readonly Field[], config: unknown): FormValues {
  const held = isRecord(config) ? config : {};
  const values: FormValues = {};
  for (const field of fields) {
    if (isSecret(field)) {
      values[field.name] = field.type === 'secret_map' ? {} : '';
      continue;
    }
    const given = held[field.name];
    switch (field.type) {
      case 'boolean':
        values[field.name] = typeof given === 'boolean' ? given : field.default === true;
        break;
      case 'map':
        values[field.name] = isRecord(given) ? Object.fromEntries(Object.entries(given).map(([name, text]) => [name, String(text)])) : {};
        break;
      default:
        values[field.name] = typeof given === 'string' ? given : typeof field.default === 'string' ? field.default : '';
    }
  }
  return values;
}

const isEmpty = (value: FormValue | undefined): boolean =>
  value === undefined || (typeof value === 'string' && value.trim() === '') || (typeof value === 'object' && Object.keys(value).length === 0);

/** The config of a channel from the values of its form: every field that is not a secret, a field left empty left out. */
export function configOf(fields: readonly Field[], values: FormValues): Record<string, unknown> {
  const config: Record<string, unknown> = {};
  for (const field of fields) {
    if (isSecret(field)) continue;
    const value = values[field.name];
    if (field.type === 'boolean') config[field.name] = value === true;
    else if (!isEmpty(value)) config[field.name] = typeof value === 'string' ? value.trim() : value;
  }
  return config;
}

/** The secrets that were typed, by field; a map of secrets goes as the text of its JSON. Nothing for a field that was left empty. */
export function secretsOf(fields: readonly Field[], values: FormValues): Record<string, string> {
  const secrets: Record<string, string> = {};
  for (const field of fields) {
    if (!isSecret(field)) continue;
    const value = values[field.name];
    if (isEmpty(value)) continue;
    secrets[field.name] = typeof value === 'string' ? value : JSON.stringify(value);
  }
  return secrets;
}

/** The names of the required fields that have nothing: a secret that is set already counts as given. */
export function missingFields(fields: readonly Field[], values: FormValues, secretsSet: readonly string[] = []): string[] {
  return fields
    .filter((field) => field.required && field.type !== 'boolean' && isEmpty(values[field.name]) && !(isSecret(field) && secretsSet.includes(field.name)))
    .map((field) => field.name);
}

/** The fields as an edit shows them: a secret that is set is not required again, and says that leaving it empty keeps it. */
export function fieldsForEdit(fields: readonly Field[], secretsSet: readonly string[], keepNote: string): Field[] {
  return fields.map((field) =>
    isSecret(field) && secretsSet.includes(field.name)
      ? { ...field, required: false, help: field.help === null ? keepNote : `${field.help} ${keepNote}` }
      : field,
  );
}
