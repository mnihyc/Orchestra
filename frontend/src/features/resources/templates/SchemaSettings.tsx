/** Searchable driver fields use the shared settings row, truthful sources and a gear for defaults. */
import { useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { RotateCcw, Search, Settings2 } from 'lucide-react';
import { record } from '@/features/resources/UnitCommands';
import { RuntimeField } from '@/features/resources/isolation/RuntimeField';
import { DetailPages, DetailSections } from '@/features/resources/DetailSections';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { HelpHint } from '@/ui/HelpHint';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { Menu } from '@/ui/Menu';
import { Select } from '@/ui/Select';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { SettingEditor } from '@/ui/SettingEditor';
import { SettingRow } from '@/ui/SettingRow';
import { Switch } from '@/ui/Switch';
import { ValueSummary } from '@/ui/ValueSummary';
import { IMAGE_SOURCES, imageSourceOf, OtherJson, SCHEMA_SECTIONS, SchemaInput, SchemaTechnicalDetails, SOURCE_MEMBERS, useFieldWords } from './SchemaForm';
import { useTemplateChoiceWords } from './choiceWords';
import { displayValue, newValue, pointerPart, pointerParts, replaceAt, resolveSchema, schemaAt, valueAt, type Schema, type SpecProblem } from './schema';

interface Field { parts: string[]; schema: Schema; defaultValue: unknown; known?: readonly string[] }
interface Props { schema: Schema; spec: Record<string, unknown>; driver: string; disabled?: boolean; readOnly?: boolean; compact?: boolean; blocked?: boolean; problems?: readonly SpecProblem[]; search?: string; toolbar?: ReactNode; leading?: (query: string) => ReactNode; onChange: (value: Record<string, unknown>) => void }
const sourceKeys = new Set<string>(IMAGE_SOURCES);

/** Recursive references stay one complex value, rather than becoming an infinite list of controls. */
function fields(root: Schema, given: unknown, value: unknown, parts: string[], fallback?: unknown, ancestors: ReadonlySet<unknown> = new Set()): Field[] {
  const raw = record(given);
  const schema = resolveSchema(root, given, value);
  const defaultValue = schema.default === undefined ? fallback : schema.default;
  const reference = typeof raw.$ref === 'string' ? raw.$ref : given;
  const properties = record(schema.properties);
  const variants = raw.anyOf ?? raw.oneOf;
  const compound = Array.isArray(variants) && variants.map((variant) => resolveSchema(root, variant)).filter((variant) => variant.type !== 'null').length > 1;
  if (Object.keys(properties).length === 0 || compound || ancestors.has(reference)) return [{ parts, schema, defaultValue }];
  const visited = new Set(ancestors); visited.add(reference);
  const children = Object.entries(properties).flatMap(([name, field]) => fields(root, field, record(value)[name], [...parts, name], record(defaultValue)[name], visited));
  if (Object.keys(record(value)).some((name) => !Object.hasOwn(properties, name))) children.push({ parts, schema: { type: 'object', additionalProperties: true }, defaultValue: undefined, known: Object.keys(properties) });
  return children;
}

function fieldValue(field: Field, spec: Record<string, unknown>): unknown {
  const current = valueAt(spec, field.parts);
  return field.known === undefined ? current : Object.fromEntries(Object.entries(record(current)).filter(([name]) => !field.known?.includes(name)));
}

function groupParts(field: Field): string[] { return field.known === undefined ? field.parts.slice(0, -1) : field.parts; }

/** Search the reference in the reader's language, including unopened list and map fields. */
function referenceWords(root: Schema, given: unknown, parts: string[], words: ReturnType<typeof useFieldWords>, choices: ReturnType<typeof useTemplateChoiceWords>, seen: ReadonlySet<unknown> = new Set()): string {
  const raw = record(given);
  const reference = typeof raw.$ref === 'string' ? raw.$ref : given;
  const field = resolveSchema(root, given);
  const own = [words(parts, field, 'title'), words(parts, field, 'description'), ...(Array.isArray(field.enum) ? field.enum.map((entry) => choices(parts, entry, field).name) : [])].join(' ');
  if (seen.has(reference)) return own;
  const visited = new Set(seen); visited.add(reference);
  return [own, ...Object.entries(record(field.properties)).map(([name, child]) => referenceWords(root, child, [...parts, name], words, choices, visited)),
    ...(field.items === undefined ? [] : [referenceWords(root, field.items, [...parts, '0'], words, choices, visited)])].join(' ');
}

function FieldRow({ field, root, value, driver, disabled, readOnly, problems, onChange }: { field: Field; root: Schema; value: unknown; driver: string; disabled: boolean; readOnly: boolean; problems: readonly SpecProblem[]; onChange: (next: unknown) => void }) {
  const { t } = useTranslation();
  const words = useFieldWords(driver);
  const choiceWords = useTemplateChoiceWords(driver);
  const [open, setOpen] = useState(false);
  // Clearing a displayed default must leave room to type its replacement before blur restores it.
  const [draft, setDraft] = useState<string | null>(null);
  const { schema, parts, defaultValue } = field;
  const title = field.known === undefined ? words(parts, schema, 'title') : t('templates.editor.other');
  const description = words(parts, schema, 'description');
  const unit = typeof schema['x-unit'] === 'string' ? schema['x-unit'] : '';
  const inherited = defaultValue === null ? undefined : defaultValue;
  const effective = value === undefined ? inherited : value;
  const choices: unknown[] | undefined = Array.isArray(schema.enum) ? schema.enum : undefined;
  const childSchema = schema.type === 'array' ? resolveSchema(root, schema.items) : schema;
  const childParts = schema.type === 'array' ? [...parts, '0'] : parts;
  const summaryWord = (entry: string) => choices !== undefined ? choiceWords(parts, entry, schema).name : Array.isArray(childSchema.enum) ? choiceWords(childParts, entry, childSchema).name : entry;
  const summaryField = (name: string) => Object.hasOwn(record(childSchema.properties), name) ? words([...childParts, name], schemaAt(root, [...childParts, name]), 'title') : name;
  const complex = schema.type === 'array' || schema.type === 'object' || schema.properties !== undefined || schema['x-multiline'] === true;
  const pointer = `/${parts.map(pointerPart).join('/')}`;
  const matching = problems.filter((problem) => {
    if (field.known !== undefined) { const name = pointerParts(problem.pointer)[parts.length]; return problem.pointer.startsWith(`${pointer}/`) && name !== undefined && !field.known.includes(name); }
    return problem.pointer === pointer || complex && problem.pointer.startsWith(`${pointer}/`);
  });
  const error = matching.map((problem) => problem.message).join(' ');
  const readable = (entry: unknown): string => entry === null || entry === undefined ? t('templateSettings.automatic') : typeof entry === 'boolean' ? t(entry ? 'common.on' : 'common.off')
    : Array.isArray(entry) ? entry.length === 0 ? t('day100.settings.empty') : entry.map((item) => typeof item === 'string' ? summaryWord(item) : displayValue(item)).join(', ')
      : typeof entry === 'object' ? t('settings.compact.fields', { count: Object.keys(record(entry)).length })
        : `${choices === undefined ? displayValue(entry) : choiceWords(parts, entry, schema).name}${unit === '' ? '' : ` ${unit}`}`;
  const source = value !== undefined ? t('templateSettings.source.template') : inherited !== undefined ? t('templateSettings.source.driver') : t('templateSettings.source.automatic');
  const range = t('templates.editor.range', { min: schema.minimum ?? '−∞', max: schema.maximum ?? '∞', unit });
  const row = { as: 'div' as const, name: title, description, settingKey: pointer, readOnly,
    reason: <Badge><span data-template-source>{source}</span></Badge>,
    help: Array.isArray(schema.examples) || schema.minimum !== undefined || schema.maximum !== undefined ? <HelpHint term={title} what={<>
      {(schema.minimum !== undefined || schema.maximum !== undefined) && <p>{range}</p>}
      {Array.isArray(schema.examples) && <p>{t('templates.editor.exampleValue', { value: schema.examples.map(displayValue).join('; ') })}</p>}
    </>} /> : undefined,
    actions: <Menu label={t('settings.editor.menuOf', { name: title })} align="end" trigger={<IconButton size="sm" label={t('settings.editor.menuOf', { name: title })} icon={<Settings2 aria-hidden />} />} entries={[
      { type: 'label', id: 'default', label: t('templates.editor.default', { value: readable(defaultValue) }) },
      ...(!readOnly && value !== undefined ? [{ id: 'reset', name: t('templates.editor.reset'), icon: <RotateCcw aria-hidden />, ...(disabled ? { disabled: t('settings.editor.saving') } : {}), onSelect: () => { setDraft(null); onChange(undefined); } }] : []),
    ]} />,
    status: error !== '' && (complex || schema.type === 'boolean') && !open ? <p role="status" className="text-sm text-danger-text">{error}</p> : undefined,
  };
  const summary = <ValueSummary inControl value={effective} word={summaryWord} fieldName={summaryField} />;
  const context = field.known === undefined ? '' : words(parts, schemaAt(root, parts), 'title');
  const editLabel = t('settings.compact.editValue', { name: context === '' ? title : `${context}: ${title}` });
  if (readOnly) return <SettingRow {...row} value={effective === undefined ? t('templateSettings.automatic') : <ValueSummary value={effective} word={summaryWord} fieldName={summaryField} number={(number) => `${number}${unit === '' ? '' : ` ${unit}`}`} />} />;
  if (schema['x-runtime'] === true) return <SettingRow {...row} value={<RuntimeField driver={driver} label={title} hideLabel value={effective} onChange={onChange} disabled={disabled} error={error || undefined} />} />;
  if (complex) return <SettingEditor {...row} label={editLabel} summary={effective === undefined ? t('templateSettings.automatic') : summary} open={open} onOpenChange={setOpen} disabled={disabled} pauseEffects={false}>
    <SchemaTechnicalDetails value={false}><SchemaInput root={root} schema={schema} value={effective} parts={parts} driver={driver} disabled={disabled} problems={problems} embedded hideHeading onChange={onChange} /></SchemaTechnicalDetails>
  </SettingEditor>;
  const common = { label: title, hideLabel: true, disabled, error: error || undefined };
  const selected = choices?.findIndex((entry) => entry === effective) ?? -1;
  return <SettingRow {...row} value={schema.type === 'boolean' ? <Switch label={<><span className="sr-only">{title}</span><span aria-hidden>{t(effective === true ? 'common.on' : 'common.off')}</span></>} checked={effective === true} disabled={disabled} onCheckedChange={onChange} />
    : choices !== undefined ? <ChoiceLocator {...common} value={selected < 0 ? '' : String(selected)} valueLabel={selected < 0 && effective !== undefined ? readable(effective) : undefined} placeholder={t('templateSettings.automatic')} options={choices.map((entry, index) => ({ value: String(index), searchText: displayValue(entry), ...choiceWords(parts, entry, schema) }))} onValueChange={(next) => onChange(choices[Number(next)])} />
      : schema.type === 'number' || schema.type === 'integer' ? <Input {...common} type="number" value={draft ?? (typeof effective === 'number' ? effective : '')} min={typeof schema.minimum === 'number' ? schema.minimum : undefined} max={typeof schema.maximum === 'number' ? schema.maximum : undefined} step={schema.type === 'integer' ? 1 : 'any'} trailing={unit === '' ? undefined : <span className="text-xs text-text-muted">{unit}</span>} onChange={(event) => { setDraft(event.target.value); onChange(event.target.value === '' ? undefined : Number(event.target.value)); }} onBlur={() => setDraft(null)} />
        : <Input {...common} value={draft ?? (typeof effective === 'string' ? effective : '')} onChange={(event) => { setDraft(event.target.value); onChange(event.target.value === '' ? undefined : event.target.value); }} onBlur={() => setDraft(null)} />} />;
}

export function SchemaForm({ schema, spec, driver, disabled = false, readOnly = false, compact = false, blocked = false, problems = [], search: suppliedSearch, toolbar, leading, onChange }: Props) {
  const { t } = useTranslation();
  const words = useFieldWords(driver);
  const choices = useTemplateChoiceWords(driver);
  const [localSearch, setSearch] = useState('');
  const search = suppliedSearch ?? localSearch;
  const tokens = search.trim().normalize('NFKC').toLocaleLowerCase().split(/\s+/).filter(Boolean);
  const root = resolveSchema(schema, schema, spec);
  const properties = record(root.properties);
  const sourceChoices = IMAGE_SOURCES.every((name) => Object.hasOwn(properties, name));
  const source = imageSourceOf(spec);
  const groups = SCHEMA_SECTIONS.map(([section, names]) => ({ id: section, title: t(`templates.editor.sections.${section}`), names: names.filter((name) => Object.hasOwn(properties, name)) }));
  const grouped = new Set<string>(SCHEMA_SECTIONS.flatMap(([, names]) => [...names]));
  const rest = Object.keys(properties).filter((name) => !grouped.has(name));
  const sections = [...groups, ...(rest.length === 0 ? [] : [{ id: 'extra', title: t('templateSettings.more'), names: rest }])];
  const matches = (field: Field, section: string) => {
    if (tokens.length === 0) return true;
    const parents = field.parts.slice(0, -1).map((_, index) => { const path = field.parts.slice(0, index + 1); return words(path, schemaAt(schema, path, spec), 'title'); });
    const text = [section, ...parents, field.parts.join('.'), field.known === undefined ? referenceWords(schema, field.schema, field.parts, words, choices) : t('templates.editor.other'), displayValue(fieldValue(field, spec))].join(' ').normalize('NFKC').toLocaleLowerCase();
    return tokens.every((token) => text.includes(token));
  };
  const rows = sections.map((section) => ({ ...section, fields: section.names.flatMap((name) => fields(schema, properties[name], spec[name], [name], record(root.default)[name])) }));
  const visible = (field: Field, section: string) => matches(field, section) && !(field.parts[0] === 'init' && (spec.init === null || spec.init === undefined)) && (!sourceChoices || !sourceKeys.has(field.parts[0] ?? '') || field.parts[0] === source);
  const imageVisible = sourceChoices && rows.some((section) => section.id === 'image' && section.fields.some((field) => matches(field, section.title)));
  const matchingCount = rows.reduce((count, section) => count + section.fields.filter((field) => visible(field, section.title)).length, imageVisible ? 1 : 0);
  const unknown = Object.fromEntries(Object.entries(spec).filter(([name]) => !Object.hasOwn(properties, name)));
  const otherMatches = Object.keys(unknown).length > 0 && tokens.every((token) => `${t('templates.editor.other')} ${displayValue(unknown)}`.normalize('NFKC').toLocaleLowerCase().includes(token));
  const leadingContent = leading?.(search);
  const renderSection = (section: (typeof rows)[number]) => {
    const groupedFields = new Map<string, { title: string; fields: Field[] }>();
    for (const field of section.fields) {
      const path = groupParts(field); const key = JSON.stringify(path);
      const group: { title: string; fields: Field[] } = groupedFields.get(key) ?? { title: path.map((_, at) => { const current = path.slice(0, at + 1); return words(current, schemaAt(schema, current, spec), 'title'); }).join(t('resources.home.pathJoin')), fields: [] };
      group.fields.push(field); groupedFields.set(key, group);
    }
    const namedByRow = section.fields.some((field) => field.known === undefined && words(field.parts, field.schema, 'title') === section.title);
    return <section key={section.id} aria-label={section.title} hidden={!section.fields.some((field) => visible(field, section.title)) && !(section.id === 'image' && imageVisible)} className="min-w-0">
      {!compact && !namedByRow && <h3 className="px-3 py-2 font-semibold">{section.title}</h3>}
      {section.id === 'image' && imageVisible && <SettingRow as="div" name={t('templates.editor.imageSource')} readOnly={readOnly} value={readOnly ? t(`templates.sourceChoices.${source}.name`) : <Select label={t('templates.editor.imageSource')} hideLabel value={source} disabled={disabled} options={IMAGE_SOURCES.map((value) => ({ value, name: t(`templates.sourceChoices.${value}.name`), description: t(`templates.sourceChoices.${value}.description`) }))} onValueChange={(next) => onChange({ ...Object.fromEntries(Object.entries(spec).filter(([name]) => !SOURCE_MEMBERS.has(name))), [next]: newValue(schema, properties[next]) })} />} />}
      {[...groupedFields].map(([key, group]) => <div key={key} role={group.title === '' ? undefined : 'group'} aria-label={group.title === '' ? undefined : group.title} hidden={!group.fields.some((field) => visible(field, section.title))}>
        {group.title !== '' && group.title !== section.title && <h4 className="px-3 pt-2 text-sm font-medium">{group.title}</h4>}
        <ul className="divide-y divide-border">{group.fields.map((field) => <li key={JSON.stringify([field.parts, field.known === undefined])} hidden={!visible(field, section.title)}>
          <FieldRow field={field} root={schema} value={fieldValue(field, spec)} driver={driver} disabled={disabled} readOnly={readOnly} problems={problems} onChange={(next) => {
            const kept = field.known === undefined ? next : { ...Object.fromEntries(Object.entries(record(valueAt(spec, field.parts))).filter(([name]) => field.known?.includes(name))), ...Object.fromEntries(Object.entries(record(next)).filter(([name]) => !field.known?.includes(name))) };
            onChange(replaceAt(spec, field.parts, kept));
          }} />
        </li>)}</ul>
      </div>)}
    </section>;
  };
  return <div className="flex min-w-0 flex-col gap-4" data-template-settings>
    <div className="flex flex-wrap items-center gap-3">
      {suppliedSearch === undefined && <Input type="search" label={t('templateSettings.search')} hideLabel placeholder={t('templateSettings.search')} value={search} disabled={compact && blocked} fieldClassName="min-w-0 flex-1" onChange={(event) => setSearch(event.target.value)} trailing={<Search aria-hidden size={16} />} />}
      {toolbar}
    </div>
    {leadingContent}
    {tokens.length > 0 && problems.length > 0 && suppliedSearch === undefined && <Button size="sm" className="self-start" onClick={() => setSearch('')}>{t('templateSettings.showAll')}</Button>}
    {tokens.length > 0 && matchingCount === 0 && !otherMatches && (leadingContent === null || leadingContent === undefined) && <EmptyState compact title={t('templateSettings.noMatch')} />}
    {compact ? <DetailSections label={t('templates.spec.title')} blocked={blocked} sections={rows.filter((section) => section.fields.some((field) => visible(field, section.title)) || section.id === 'image' && imageVisible).map((section) => ({ value: section.id, label: section.title,
      panel: <DetailPages blocked={blocked} items={section.fields.filter((field) => visible(field, section.title))}>{(page) => renderSection({ ...section, fields: [...page] })}</DetailPages>,
    }))} /> : rows.map(renderSection)}
    {Object.keys(unknown).length > 0 && <div hidden={!otherMatches}>{readOnly ? <SettingRow as="div" name={t('templates.editor.other')} readOnly reason={<Badge>{t('templateSettings.source.template')}</Badge>} value={<ValueSummary value={unknown} />} />
      : <SettingEditor as="div" name={t('templates.editor.other')} label={t('settings.compact.editValue', { name: t('templates.editor.other') })} reason={<Badge>{t('templateSettings.source.template')}</Badge>} summary={<ValueSummary value={unknown} inControl />} pauseEffects={false}
          status={problems.filter((problem) => Object.hasOwn(unknown, pointerParts(problem.pointer)[0] ?? '')).map((problem) => <p key={JSON.stringify(problem)} role="status" className="text-sm text-danger-text">{problem.message}</p>)}>
        <SchemaTechnicalDetails value={false}><OtherJson objectOnly value={unknown} disabled={disabled} onChange={(next) => onChange({ ...Object.fromEntries(Object.entries(spec).filter(([name]) => Object.hasOwn(properties, name))), ...Object.fromEntries(Object.entries(record(next)).filter(([name]) => !Object.hasOwn(properties, name))) })} /></SchemaTechnicalDetails>
      </SettingEditor>}</div>}
  </div>;
}
