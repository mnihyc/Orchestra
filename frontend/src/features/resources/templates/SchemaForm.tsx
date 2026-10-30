/** Accessible schema fields retain every unrecognised member while editing the fields the driver explains. */
import { createContext, useContext, useEffect, useId, useState } from 'react';
import { RotateCcw } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { record } from '@/features/resources/UnitCommands';
import { ResourceDetails } from '@/features/resources/DetailSections';
import { displayValue, fieldPath, newValue, pointerPart, replaceMember, resolveSchema, type Schema, type SpecProblem } from '@/features/resources/templates/schema';
import { useTemplateChoiceWords } from '@/features/resources/templates/choiceWords';
import { RuntimeField } from '@/features/resources/isolation/RuntimeField';
import { Button } from '@/ui/Button';
import { HelpHint } from '@/ui/HelpHint';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { Switch } from '@/ui/Switch';
import { Textarea } from '@/ui/Textarea';

/** Sections follow the Overview. Plugin fields without a known section stay in their declared order. */
export const SCHEMA_SECTIONS = [
  ['image', ['image', 'image_digest', 'build', 'archive']],
  ['confinement', ['runtime', 'allowed_runtimes', 'security', 'sandbox', 'base_capabilities', 'allowed_capabilities']],
  ['uses', ['resources', 'resources_max', 'needs', 'requests']],
  ['network', ['network']], ['exec', ['exec', 'colocated_harnesses', 'mcp_server_ids', 'agent_defaults']],
  ['user', ['user', 'run_as']], ['variables', ['env']], ['commands', ['after_up', 'init']],
  ['storage', ['workspace_mount', 'home_mount', 'mounts']], ['needs', ['host_labels']], ['programs', ['plugins']],
] as const;
export const IMAGE_SOURCES = ['image', 'build', 'archive'] as const;
export const SOURCE_MEMBERS = new Set<string>([...IMAGE_SOURCES, 'image_digest']);
export const imageSourceOf = (spec: Record<string, unknown>): (typeof IMAGE_SOURCES)[number] => spec.build !== null && spec.build !== undefined ? 'build' : spec.archive !== null && spec.archive !== undefined ? 'archive' : 'image';

/** A malformed auxiliary JSON draft must be fixed before a view switch or save. */
export const SchemaDraftState = createContext<(id: string, invalid: boolean) => void>(() => undefined);
/** Resource details keep unshaped plugin values behind the same technical disclosure. */
export const SchemaTechnicalDetails = createContext(false);

export function useFieldWords(driver: string) {
  const { t, i18n } = useTranslation();
  return (parts: readonly string[], schema: Schema, member: 'title' | 'description' | 'name'): string => {
    const key = `templates.fields.${fieldPath(parts)}.${member}`;
    const specific = `templates.driverFields.${driver}.${fieldPath(parts)}.${member}`;
    if (i18n.exists(specific)) return t(specific, { defaultValue: typeof schema[member] === 'string' ? schema[member] : '' });
    // A different driver can reuse a property name with a different meaning.
    if ((i18n.exists(`templates.driverChoices.${driver}.name`) || i18n.exists(`templates.driverFields.${driver}.description`)) && i18n.exists(key)) return t(key, { defaultValue: typeof schema[member] === 'string' ? schema[member] : '' });
    return typeof schema[member] === 'string' ? schema[member] : member === 'name' && typeof schema.title === 'string' ? schema.title : member === 'description' ? '' : parts.at(-1)?.replaceAll('_', ' ') ?? '';
  };
}

interface Props { root: Schema; schema: unknown; value: unknown; parts: string[]; disabled: boolean; problems: readonly SpecProblem[]; driver: string; onChange: (value: unknown) => void; embedded?: boolean; hideHeading?: boolean }

/** Raw values with no form shape remain editable, including allowed plugin extensions. */
export function OtherJson({ value, disabled, onChange, objectOnly = false }: { value: unknown; disabled: boolean; onChange: (value: unknown) => void; objectOnly?: boolean }) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<string | null>(null);
  const [error, setError] = useState<string>();
  const id = useId();
  const reportDraft = useContext(SchemaDraftState);
  const technicalDetails = useContext(SchemaTechnicalDetails);
  useEffect(() => () => reportDraft(id, false), [reportDraft, id]);
  const editor = <Textarea label={t('templates.editor.other')} rows={5} className="font-mono" value={draft ?? JSON.stringify(value ?? {}, null, 2)} disabled={disabled} error={error} onChange={(event) => {
    const typed = event.target.value;
    setDraft(typed);
    try {
      const next: unknown = JSON.parse(typed);
      if (objectOnly && (next === null || typeof next !== 'object' || Array.isArray(next))) { setError(t('templates.editor.invalidJson')); reportDraft(id, true); return; }
      onChange(next); setError(undefined); reportDraft(id, false);
    } catch { setError(t('templates.editor.invalidJson')); reportDraft(id, true); }
  }} onBlur={() => { if (error === undefined) setDraft(null); }} />;
  return technicalDetails ? <ResourceDetails keepMounted pauseEffects={false}>{editor}</ResourceDetails> : editor;
}

export function SchemaInput({ root, schema: given, value, parts, disabled, problems, driver, onChange, embedded = false, hideHeading = false }: Props) {
  const { t, i18n } = useTranslation();
  const words = useFieldWords(driver);
  const enumWords = useTemplateChoiceWords(driver);
  const errorId = useId();
  const schema = resolveSchema(root, given, value);
  const label = typeof schema.title !== 'string' && /^\d+$/.test(parts.at(-1) ?? '') ? t('templates.editor.itemNumber', { number: Number(parts.at(-1)) + 1 }) : words(parts, schema, schema.type==='boolean'?'name':'title');
  const description = words(parts, schema, 'description');
  const pointer = `/${parts.map(pointerPart).join('/')}`;
  const items = resolveSchema(root, schema.items);
  const structuredList = items.properties !== undefined || items.type === 'object' || items.type === 'array' || Array.isArray(items.enum);
  const error = problems.filter((problem) => problem.pointer === pointer || schema.type === 'array' && !structuredList && problem.pointer.startsWith(`${pointer}/`)).map((problem) => problem.message).join(' ');
  const defaultValue = schema.default;
  const unit = typeof schema['x-unit'] === 'string' ? schema['x-unit'] : '';
  const printable = displayValue;
  const defaultWords = t('templates.editor.default', { value: defaultValue === null || defaultValue === undefined ? t('templates.editor.unset') : typeof defaultValue === 'boolean' ? t(defaultValue ? 'templates.editor.on' : 'templates.editor.off') : `${Array.isArray(schema.enum)?enumWords(parts,defaultValue,schema).name:printable(defaultValue)}${unit === '' ? '' : ` ${unit}`}` });
  const rangeWords = t('templates.editor.range', { min: schema.minimum ?? '−∞', max: schema.maximum ?? '∞', unit });
  const reset = embedded || value === undefined ? undefined : <IconButton size="sm" disabled={disabled} onClick={() => onChange(undefined)} label={t('templates.editor.resetField', { name: label })} icon={<RotateCcw aria-hidden />} />;
  const hints = <>{description!==''&&<p className="text-sm text-text-muted">{description}</p>}<p className="text-xs text-text-subtle">{defaultWords}</p>{Array.isArray(schema.examples) && <p className="text-xs wrap-anywhere text-text-subtle">{t('templates.editor.exampleValue', { value: schema.examples.map(printable).join('; ') })}</p>}
    {(schema.minimum !== undefined || schema.maximum !== undefined) && <p className="text-xs text-text-subtle">{rangeWords}</p>}</>;
  const help = embedded ? undefined : <HelpHint term={label} what={hints} />;
  const child = (name: string, field: unknown, childValue: unknown, change: (next: unknown) => void) => <SchemaInput key={name} root={root} schema={field} value={childValue} parts={[...parts, name]} disabled={disabled} problems={problems} driver={driver} onChange={change} embedded={embedded} />;

  if (schema.properties !== undefined || schema.type === 'object') {
    const properties = record(schema.properties);
    const current = record(value);
    const additional = resolveSchema(root, schema.additionalProperties);
    const stringMap = Object.keys(properties).length === 0 && additional.type === 'string';
    const unknown = Object.fromEntries(Object.entries(current).filter(([key]) => !Object.hasOwn(properties, key)));
    const variants = record(given).anyOf ?? record(given).oneOf;
    const alternatives = Array.isArray(variants) ? variants.map((variant) => resolveSchema(root, variant)).filter((variant) => variant.type === 'object') : [];
    const variantWords=(variant:Schema)=>{
      const declared=record(variant['x-choice']);
      const fallbackName=typeof declared.name==='string'?declared.name:typeof variant.title==='string'?variant.title:Object.keys(record(variant.properties)).join(', ');
      const fallbackDescription=typeof declared.description==='string'?declared.description:typeof variant.description==='string'?variant.description:'';
      const nameKey=`templates.contextChoices.${typeof declared.id==='string'?declared.id:''}.name`;
      const descriptionKey=`templates.contextChoices.${typeof declared.id==='string'?declared.id:''}.description`;
      const known=i18n.exists(`templates.driverChoices.${driver}.name`) || i18n.exists(`templates.driverFields.${driver}.description`);
      return {name:known&&i18n.exists(nameKey)?t(nameKey,{defaultValue:fallbackName}):fallbackName,description:known&&i18n.exists(descriptionKey)?t(descriptionKey,{defaultValue:fallbackDescription}):fallbackDescription};
    };
    return <fieldset className="min-w-0 flex flex-col gap-3 rounded-lg border border-border p-3" disabled={disabled} aria-label={label} aria-describedby={error ? errorId : undefined}>
      {!hideHeading && <legend className="px-1"><span className="inline-flex items-center gap-1 font-medium">{label}{help}{reset}</span></legend>}
      {alternatives.length > 1 && <ChoiceLocator label={t('templates.editor.shape')} value={String(Math.max(0, alternatives.findIndex((variant) => variant.properties === schema.properties)))} options={alternatives.map((variant, index) => ({value:String(index),searchText:'',...variantWords(variant)}))} onValueChange={(index) => onChange(newValue(root, alternatives[Number(index)]))} disabled={disabled} />}
      {Object.entries(properties).map(([name, field]) => child(name, field, current[name], (next) => { const updated = replaceMember(current, name, next); onChange(Object.keys(updated).length === 0 ? undefined : updated); }))}
      {stringMap ? <div className="flex flex-col gap-3">{Object.entries(current).map(([name, entry], index) => <div key={index} className="flex flex-col gap-2 rounded-lg border border-border p-2">
        <Input label={t('templates.editor.key')} value={name} error={Object.hasOwn(current, '') && name === '' ? t('templates.editor.keyRequired') : undefined} onChange={(event) => {
          if (event.target.value !== name && Object.hasOwn(current, event.target.value)) return;
          onChange(Object.fromEntries(Object.entries(current).map(([key, value]) => [key === name ? event.target.value : key, value])));
        }} />
        {child(name, { ...additional, title: t('templates.editor.value') }, entry, (next) => onChange(replaceMember(current, name, next)))}
        <Button size="sm" variant="ghost" onClick={() => onChange(replaceMember(current, name, undefined))}>{t('templates.editor.remove')}</Button>
      </div>)}<Button size="sm" disabled={disabled || Object.hasOwn(current, '')} onClick={() => onChange({ ...current, '': '' })}>{t('templates.editor.addPair')}</Button></div> : (Object.keys(unknown).length > 0 || Object.keys(properties).length === 0) && <OtherJson objectOnly value={Object.keys(properties).length === 0 ? value : unknown} disabled={disabled} onChange={(next) => {
        if (Object.keys(properties).length === 0) onChange(next);
        else onChange({ ...Object.fromEntries(Object.entries(current).filter(([key]) => Object.hasOwn(properties, key))), ...Object.fromEntries(Object.entries(record(next)).filter(([key]) => !Object.hasOwn(properties, key))) });
      }} />}
      {error && <p id={errorId} role="status" className="text-sm text-danger-text">{error}</p>}
    </fieldset>;
  }
  if (schema.type === 'array') {
    const list: unknown[] = Array.isArray(value) ? value : [];
    const structured = structuredList;
    return <fieldset className="min-w-0 flex flex-col gap-3 rounded-lg border border-border p-3" disabled={disabled} aria-label={label}>
      {!hideHeading && <legend className="px-1"><span className="inline-flex items-center gap-1 font-medium">{label}{help}{reset}</span></legend>}
      {structured ? <>{list.map((entry, index) => <div key={index} className="flex min-w-0 flex-col gap-2">{child(String(index), schema.items, entry, (next) => onChange(list.map((old, position) => position === index ? next : old)))}<Button size="sm" variant="ghost" onClick={() => onChange(list.filter((_, position) => position !== index))}>{t('templates.editor.remove')}</Button></div>)}<Button size="sm" onClick={() => onChange([...list, newValue(root, schema.items)])}>{t('templates.editor.addItem')}</Button></> : <Textarea label={t('templates.editor.listValues', { name: label })} hideLabel rows={3} value={list.map(String).join('\n')} placeholder={embedded ? undefined : defaultWords} error={error || undefined} onChange={(event) => onChange(event.target.value === '' ? [] : event.target.value.split('\n').map((entry) => items.type === 'integer' || items.type === 'number' ? Number(entry) : items.type === 'boolean' ? entry === 'true' : entry))} />}
      {error && structured && <p role="status" className="text-sm text-danger-text">{error}</p>}
    </fieldset>;
  }
  const common = { label, disabled, error: error || undefined, placeholder: embedded ? undefined : defaultWords, aside: reset, help };
  // A runtime is chosen among those the hosts report, or named by one who knows the host has it.
  if (schema['x-runtime'] === true) return <RuntimeField driver={driver} label={label} value={value} onChange={onChange} disabled={disabled} error={error || undefined} help={help} aside={reset} placeholder={embedded ? undefined : t('templates.editor.unset')} />;
  const choices: unknown[] | undefined = Array.isArray(schema.enum) ? schema.enum : undefined;
  const selectedChoice=choices?.findIndex((entry)=>entry===value)??-1;
  return <div className="flex flex-col gap-1">
    {schema.type === 'boolean' ? <><div className="flex flex-wrap items-center gap-1"><Switch name={label} description={description} checked={typeof value === 'boolean' ? value : defaultValue === true} disabled={disabled} onCheckedChange={onChange} />{help}{reset}</div>{error && <p role="status" className="text-sm text-danger-text">{error}</p>}</> :
      choices !== undefined ? <ChoiceLocator {...common} value={selectedChoice<0?'':String(selectedChoice)} valueLabel={selectedChoice < 0 && value !== undefined ? printable(value) : undefined} options={choices.map((entry,index) => ({value:String(index),searchText:printable(entry),...enumWords(parts,entry,schema)}))} onValueChange={(next) => onChange(choices[Number(next)])} /> :
      schema.type === 'number' || schema.type === 'integer' ? <Input {...common} type="number" value={typeof value === 'number' ? value : ''} min={typeof schema.minimum === 'number' ? schema.minimum : undefined} max={typeof schema.maximum === 'number' ? schema.maximum : undefined} step={schema.type === 'integer' ? 1 : 'any'} trailing={unit && <span className="text-xs text-text-muted">{unit}</span>} onChange={(event) => onChange(event.target.value === '' ? undefined : Number(event.target.value))} /> :
      schema['x-multiline'] === true ? <Textarea {...common} rows={8} value={typeof value === 'string' ? value : ''} onChange={(event) => onChange(event.target.value === '' ? undefined : event.target.value)} /> :
      schema.type === 'string' || schema.type === undefined ? <Input {...common} value={typeof value === 'string' ? value : ''} onChange={(event) => onChange(event.target.value === '' ? undefined : event.target.value)} /> :
      <OtherJson value={value} disabled={disabled} onChange={onChange} />}
  </div>;
}

