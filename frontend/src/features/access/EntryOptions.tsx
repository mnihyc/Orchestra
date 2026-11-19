/** What a holder's entry on a resource carries: models, efforts and arrangement types on a provider, the limits of a host and of a template, and expiry.
 *
 * An entry may end on a day, and on a resource it may carry options that narrow what its holder gets:
 * the models and efforts they may run, in which arrangements, how many environments they may have on
 * a host, what one may be given. The options are drawn from the JSON Schema the catalogue gives the
 * permission, so a permission that gains an option gains its field with no code that names it. A
 * member left empty is left out, and a member that is left out does not narrow what the owners allow.
 */
import { useState } from 'react';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import { offeredChoice } from '@/i18n/choices';

import type { FileReachFacts, Holder, PermissionDef, PermissionRow } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { DateField } from '@/ui/DateField';
import type { WhereName } from '@/generated/api-types';

import type { Changes } from './changes';
import { editFollowedHolderChange, holderKey, updateEntryChange } from './model';
import { permissionNameOf } from './names';
import { WhereField, whereNames } from './WhereField';

type Json = Record<string, unknown>;

/** One option an entry may carry, as a field. */
export type OptionField =
  | { key: string; kind: 'choice'; choices: readonly string[]; defaultValue: string }
  | { key: string; kind: 'strings' }
  | { key: string; kind: 'choices'; choices: readonly { value: string; hint: string; leaves: boolean; defaultSelected?: boolean }[] }
  | { key: string; kind: 'integer' }
  | { key: string; kind: 'places' }
  | { key: string; kind: 'limits'; fields: readonly OptionField[] };

const isRecord = (value: unknown): value is Json => typeof value === 'object' && value !== null && !Array.isArray(value);
const typesOf = (schema: Json): string[] => (Array.isArray(schema.type) ? schema.type.filter((entry): entry is string => typeof entry === 'string') : typeof schema.type === 'string' ? [schema.type] : []);

/** A `$ref` into the schema's own definitions, followed. */
function follow(root: Json, schema: unknown): Json | null {
  if (!isRecord(schema)) return null;
  const ref = typeof schema.$ref === 'string' ? /^#\/\$defs\/(.+)$/.exec(schema.$ref)?.[1] : undefined;
  const defs = isRecord(root.$defs) ? root.$defs : {};
  if (ref !== undefined) return isRecord(defs[ref]) ? defs[ref] : null;
  return schema;
}

/** The fields of the options a permission allows, from the schema the catalogue gives it. Nothing for a permission that has none. */
export function optionFields(schema: unknown, root: Json | null = isRecord(schema) ? schema : null): OptionField[] {
  if (!isRecord(schema) || root === null || !isRecord(schema.properties)) return [];
  const fields: OptionField[] = [];
  for (const [key, raw] of Object.entries(schema.properties)) {
    const member = follow(root, raw) ?? (isRecord(raw) ? raw : null);
    if (member === null) continue;
    const types = typesOf(member);
    if (types.includes('string') && Array.isArray(member.enum) && member.enum.every((value) => typeof value === 'string')) {
      const initial = isRecord(raw) && typeof raw.default === 'string' ? raw.default : typeof member.default === 'string' ? member.default : '';
      fields.push({ key, kind: 'choice', choices: member.enum, defaultValue: initial });
    } else if (types.includes('integer')) fields.push({ key, kind: 'integer' });
    else if (types.includes('array')) {
      const items = follow(root, member.items);
      if (items !== null && isRecord(items.properties) && 'kind' in items.properties && 'id' in items.properties) fields.push({ key, kind: 'places' });
      else if (items !== null && Array.isArray(items.oneOf)) {
        const choices = items.oneOf.flatMap((entry) => (isRecord(entry) && typeof entry.const === 'string' ? [{ value: entry.const, hint: typeof entry.description === 'string' ? entry.description : '', leaves: entry['x-credential-leaves'] === true, defaultSelected: entry['x-default-selected'] === true }] : []));
        fields.push({ key, kind: 'choices', choices });
      } else if (items !== null && Array.isArray(items.enum) && items.enum.every((value) => typeof value === 'string')) {
        fields.push({ key, kind: 'choices', choices: items.enum.map((value: string) => ({ value, hint: '', leaves: false })) });
      } else fields.push({ key, kind: 'strings' });
    } else if (Array.isArray(member.anyOf)) {
      const inner = member.anyOf.map((entry) => follow(root, entry)).find((entry) => entry !== null && typesOf(entry).includes('object'));
      if (inner !== undefined && inner !== null) fields.push({ key, kind: 'limits', fields: optionFields(inner, root) });
    }
  }
  return fields;
}

/**
 * What an entry that leaves a choices member out has: where some choices let the credential leave the place it is kept in, only the
 * others, which are then the default. Other choices can declare their default selection in the schema.
 * `null` for a member without a declared default.
 */
export function stayingChoices(field: Extract<OptionField, { kind: 'choices' }>): string[] | null {
  if (field.choices.some((choice) => choice.leaves)) return field.choices.filter((choice) => !choice.leaves).map((choice) => choice.value);
  return field.choices.some((choice) => choice.defaultSelected) ? field.choices.filter((choice) => choice.defaultSelected).map((choice) => choice.value) : null;
}

/** What is typed, read back as the options: a member left empty is left out. */
export function readOptions(fields: readonly OptionField[], values: Json): Json {
  const out: Json = {};
  for (const field of fields) {
    const value = values[field.key];
    if (field.kind === 'choice') {
      if (typeof value === 'string' && field.choices.includes(value)) out[field.key] = value;
    } else if (field.kind === 'strings') {
      const list = typeof value === 'string' ? value.split(',').map((entry) => entry.trim()).filter((entry) => entry !== '') : [];
      if (list.length > 0) out[field.key] = list;
    } else if (field.kind === 'choices') {
      const staying = stayingChoices(field);
      const chosen = Array.isArray(value) ? field.choices.map((choice) => choice.value).filter((entry) => value.includes(entry)) : [];
      // Naming a type makes the list exclusive, so the default is written only when something else was chosen.
      if (field.key === 'purposes' ? chosen.length !== field.choices.length : staying === null ? chosen.length > 0 : chosen.length !== staying.length || chosen.some((entry) => !staying.includes(entry))) out[field.key] = chosen;
    } else if (field.kind === 'places') {
      const names = whereNames(value);
      if (names.length > 0) out[field.key] = names;
    } else if (field.kind === 'integer') {
      const number = typeof value === 'string' && value.trim() !== '' ? Number(value) : null;
      if (number !== null && Number.isInteger(number) && number >= 0) out[field.key] = number;
    } else if (isRecord(value)) {
      const inner = readOptions(field.fields, value);
      if (Object.keys(inner).length > 0) out[field.key] = inner;
    }
  }
  return out;
}

/** What an entry carries now, as the text each field is typed in. */
function valuesOf(fields: readonly OptionField[], options: unknown): Json {
  const given = isRecord(options) ? options : {};
  const out: Json = {};
  for (const field of fields) {
    const value = given[field.key];
    if (field.kind === 'choice') out[field.key] = typeof value === 'string' && field.choices.includes(value) ? value : field.defaultValue;
    else if (field.kind === 'strings') out[field.key] = Array.isArray(value) ? value.join(', ') : '';
    else if (field.kind === 'choices') out[field.key] = Array.isArray(value) ? value : field.key === 'purposes' ? field.choices.map((choice) => choice.value) : stayingChoices(field) ?? [];
    else if (field.kind === 'places') out[field.key] = whereNames(value);
    else if (field.kind === 'integer') out[field.key] = typeof value === 'number' ? String(value) : '';
    else out[field.key] = valuesOf(field.fields, value);
  }
  return out;
}

export const optionLabel = (t: TFunction, key: string): string => {
  const known = ['reach', 'purposes', 'models', 'services', 'efforts', 'arrangements', 'max_environments', 'storages', 'resources_max', 'sources', 'templates', 'capabilities', 'where', 'cpu_millis', 'memory_mb', 'memory_swap_mb', 'blkio_weight', 'pids', 'disk_gb'];
  return known.includes(key) ? t(`access.options.${key}`, { defaultValue: key }) : key.replaceAll('_', ' ').replaceAll('.', ' ');
};
const optionChoice=(t:TFunction,value:string)=>offeredChoice(t,'arrangement',value)??offeredChoice(t,'effort',value);

/** Initial sharing writes these same choices in the first grant. */
export function ProviderPurposeField({ schema, value, onChange }: { schema: unknown; value: readonly string[] | null; onChange: (value: string[] | null) => void }) {
  const { t } = useTranslation();
  const field = optionFields(schema).find((field): field is Extract<OptionField, { kind: 'choices' }> => field.key === 'purposes' && field.kind === 'choices');
  if (field === undefined) return null;
  const all = field.choices.map((choice) => choice.value), chosen = value ?? all;
  return <fieldset className="flex flex-col gap-2"><legend className="text-sm font-medium text-text">{t('access.options.purposes')}</legend>
    {field.choices.map((choice) => <Checkbox key={choice.value} presentation="compact" label={providerUsePurposeName(t,choice.value) ?? choice.value}
      checked={chosen.includes(choice.value)} onCheckedChange={(on) => {
        const next=on ? [...chosen,choice.value] : chosen.filter((value) => value !== choice.value);
        onChange(next.length === all.length ? null : next);
      }} />)}
  </fieldset>;
}

export const providerUsePurposeName = (t: TFunction, value: string): string | undefined => value === 'ask' ? t('access.options.purposeNames.ask') : value === 'work' ? t('access.options.purposeNames.work') : undefined;

function Fields({ fields, values, onChange, ports = false }: { ports?: boolean; fields: readonly OptionField[]; values: Json; onChange: (next: Json) => void }) {
  const { t } = useTranslation();
  return (
    <>
      {fields.map((field) => {
        const name = optionLabel(t, field.key);
        if (field.kind === 'choice') return <Select key={field.key} label={name} value={typeof values[field.key] === 'string' ? values[field.key] as string : field.defaultValue}
          options={field.choices.map((value) => ({ value, name: field.key === 'reach' ? (ports ? t(value === 'machine' ? 'ports.namedLoopback' : 'ports.environmentListeners') : t(`fileReach.${value}`)) : optionChoice(t, value)?.name ?? value }))}
          onValueChange={(value) => onChange({ ...values, [field.key]: value })} />;
        if (field.kind === 'limits') {
          return (
            <fieldset key={field.key} className="flex flex-col gap-3 rounded-lg border border-border p-3">
              <legend className="px-1 text-sm font-medium text-text">{name}</legend>
              <Fields fields={field.fields} values={isRecord(values[field.key]) ? values[field.key] as Json : {}} onChange={(inner) => onChange({ ...values, [field.key]: inner })} />
            </fieldset>
          );
        }
        if (field.kind === 'places') {
          const chosen: WhereName[] = whereNames(values[field.key]);
          return <WhereField key={field.key} label={name} value={chosen} onChange={(next) => onChange({ ...values, [field.key]: next })} />;
        }
        if (field.kind === 'choices') {
          const chosen = Array.isArray(values[field.key]) ? (values[field.key] as string[]) : [];
          const leaving = field.choices.filter((choice) => choice.leaves && chosen.includes(choice.value));
          return (
            <fieldset key={field.key} className="flex flex-col gap-2">
              <legend className="text-sm font-medium text-text">{name}</legend>
              {field.choices.map((choice) => (
                <Checkbox
                  key={choice.value}
                  presentation="compact"
                  label={(field.key === 'purposes' ? providerUsePurposeName(t, choice.value) : optionChoice(t,choice.value)?.name) ?? choice.value}
                  description={field.key === 'purposes' ? undefined : optionChoice(t,choice.value)?.description ?? (choice.hint === '' ? undefined : choice.hint)}
                  checked={chosen.includes(choice.value)}
                  onCheckedChange={(on) => onChange({ ...values, [field.key]: on ? [...chosen, choice.value] : chosen.filter((entry) => entry !== choice.value) })}
                />
              ))}
              {leaving.length > 0 && (
                <InlineAlert tone="warning" className="text-sm">
                  {leaving.map((choice) => <p key={choice.value}>{t(`access.options.leaves.${choice.value}`, { defaultValue: t('access.options.leavesOther') })}</p>)}
                </InlineAlert>
              )}
            </fieldset>
          );
        }
        return (
          <Input
            key={field.key}
            label={name}
            hint={field.key === 'models' ? t('access.options.modelsHint') : field.key === 'services' ? t('access.options.servicesHint') : field.kind === 'strings' ? t('access.options.listHint') : undefined}
            type={field.kind === 'integer' ? 'number' : 'text'}
            min={field.kind === 'integer' ? 0 : undefined}
            value={typeof values[field.key] === 'string' ? (values[field.key] as string) : ''}
            onChange={(event) => onChange({ ...values, [field.key]: event.target.value })}
          />
        );
      })}
    </>
  );
}

export interface EntryOptionsProps {
  open: boolean;
  onClose: () => void;
  def: PermissionDef;
  row: PermissionRow;
  entryId: string;
  /** Whom the entry names, for the heading. */
  holder: string;
  changes: Changes;
  followedHolder?: Holder;
  fileReach?: FileReachFacts | null | undefined;
}

export function EntryOptions({ open, onClose, def, row, entryId, holder, changes, followedHolder, fileReach }: EntryOptionsProps) {
  const { t } = useTranslation();
  const entry = followedHolder === undefined ? row.entries.find((candidate) => candidate.id === entryId) : row.inherited?.entries?.find((candidate) => holderKey(candidate.holder) === holderKey(followedHolder));
  const fields = optionFields(def.options_schema);
  const [values, setValues] = useState<Json>(() => valuesOf(fields, entry?.options));
  const end = entry?.expires_at === null || entry?.expires_at === undefined ? null : new Date(entry.expires_at);
  const original = end === null ? '' : new Date(end.getTime() - end.getTimezoneOffset() * 60_000).toISOString().slice(0, 16);
  const [until, setUntil] = useState(original);
  const expiresAt = until === original ? entry?.expires_at ?? null : until === '' ? null : new Date(until).toISOString();
  const permission = permissionNameOf(def);

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !changes.busy && onClose()}
      size="sm"
      title={t('access.options.title', { holder, permission })}
      description={t('access.options.lead')}
      dismissible={!changes.busy}
      footer={
        <>
          <Button disabled={changes.busy} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button
            variant="primary"
            pending={changes.busy}
            onClick={() =>
              void changes
                .run({
                  label: t('access.options.saving', { holder, permission }),
                  make: (page) =>
                    (followedHolder === undefined ? updateEntryChange(page, row.permission, row.profile, entryId, {
                      expires_at: expiresAt,
                      ...(fields.length === 0 ? {} : { options: readOptions(fields, values) }),
                    }) : editFollowedHolderChange(page, row, followedHolder, {
                      expires_at: expiresAt,
                      ...(fields.length === 0 ? {} : { options: readOptions(fields, values) }),
                    })),
                })
                .then((made) => made && onClose())
            }
          >
            {t('access.options.save')}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4 py-2">
        {def.name !== 'ports.view' && fields.some((field) => field.key === 'reach') && fileReach !== null && fileReach !== undefined &&
          <span className="text-sm text-text-muted">{t('fileReach.bound', { reach: t(`fileReach.${fileReach.machine_bound}`) })}</span>}
        <DateField type="datetime-local" label={t('access.cell.until')} value={until} onValueChange={setUntil} hint={t('access.options.untilHint')} />
        <Fields fields={fields} values={values} onChange={setValues} ports={def.name === 'ports.view'} />
        {changes.error !== null && changes.error !== undefined && <ErrorAlert error={changes.error} />}
      </div>
    </Dialog>
  );
}

/** The same current file geometry wherever a permission's existing source line appears. */
export function fileReachWords(t: TFunction, permission: string, facts: FileReachFacts | null | undefined): string | null {
  if (facts === null || facts === undefined || (permission !== 'files.view' && permission !== 'files.manage')) return null;
  const reach = permission === 'files.view' ? facts.view : facts.manage;
  if (reach === null) return null;
  const terminal = permission === 'files.view' ? facts.view_from_terminal : facts.manage_from_terminal;
  return [t(`fileReach.${reach}`), ...(terminal ? [t('fileReach.terminal')] : [])].join(t('startFlow.separator'));
}
