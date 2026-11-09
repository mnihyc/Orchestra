/** Documented options edit the existing schema draft; unfamiliar schema structure remains raw. */
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { documentedOptions, suggestedRule, type DocumentedOption } from './optionCatalogue';
import { optionProperty, setOptionProperty, type OptionProperty } from './requestOptionSchema';
import { record } from '@/features/resources/UnitCommands';
import { HelpHint } from '@/ui/HelpHint';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { Switch } from '@/ui/Switch';
import { Textarea } from '@/ui/Textarea';
import { Button } from '@/ui/Button';
import { InlineAlert } from '@/ui/InlineAlert';

type Mode = 'inherit' | 'allow' | 'block' | 'fixed' | 'remove' | 'custom';
const simpleKeys = new Set(['type', 'enum', 'default', 'const', 'items', 'x-orch-replace']);
function modeOf(property: OptionProperty, option: DocumentedOption): Mode {
  if (property.state === 'custom' || property.state === 'blocked') return 'custom';
  if (property.state === 'absent') return 'inherit';
  if (property.rule === false) return 'block';
  if (property.rule === true) return 'allow';
  if (property.rule === null || typeof property.rule !== 'object' || Array.isArray(property.rule)) return 'custom';
  const value = record(property.rule);
  if (Object.keys(value).some((key) => !simpleKeys.has(key))) return 'custom';
  if (Object.hasOwn(value, 'const')) {
    if (!option.identifier) return 'custom';
    return value.const === null && value['x-orch-replace'] === true ? 'remove' : typeof value.const === 'string' ? 'fixed' : 'custom';
  }
  if (value.enum !== undefined && (!Array.isArray(value.enum) || !value.enum.every((item) => typeof item === (option.kind === 'boolean' ? 'boolean' : 'string')))) return 'custom';
  if (option.kind === 'strings' && Object.keys(record(value.items)).some((key) => key !== 'type' && key !== 'enum')) return 'custom';
  return 'allow';
}
const split = (text: string) => [...new Set(text.split(/[,\n]/u).map((part) => part.trim()).filter((part) => part !== ''))];
const strings = (value: unknown): string[] => Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];

function OptionRow({ option, property, disabled, change, raw }: { option: DocumentedOption; property: OptionProperty; disabled: boolean; change: (rule: unknown) => void; raw: () => void }) {
  const { t } = useTranslation();
  const mode = modeOf(property, option);
  const rule = record(property.rule);
  const values = option.kind === 'strings' ? strings(record(rule.items).enum) : strings(rule.enum);
  const basis = JSON.stringify(values);
  const [draft, setDraft] = useState({ basis, text: values.join(', ') });
  const title = t(`providerFields.fields.${option.id}`);
  const updateValues = (text: string) => {
    const choices = split(text);
    setDraft({ basis: JSON.stringify(choices), text });
    change(option.kind === 'strings' ? { ...rule, ...suggestedRule(option), items: { ...record(rule.items), type: 'string', enum: choices } }
      : { ...rule, ...suggestedRule(option), enum: choices });
  };
  const updateDefault = (value: string) => {
    const next = { ...rule };
    if (value === '\u0000none') Reflect.deleteProperty(next, 'default'); else next.default = value;
    change(next);
  };
  const modes = [
    { value: 'inherit' as const, label: t('providerFields.inherit') },
    { value: 'allow' as const, label: t('providerFields.constrain') },
    { value: 'block' as const, label: t('providerFields.block') },
    ...(option.identifier ? [{ value: 'fixed' as const, label: t('providerFields.fixed') }, { value: 'remove' as const, label: t('providerFields.remove') }] : []),
    ...(mode === 'custom' ? [{ value: 'custom' as const, label: t('providerFields.custom') }] : []),
  ];
  return <div className="flex min-w-0 flex-col gap-2 border-b border-border py-3 last:border-0" data-option-field={option.path.join('.')}>
    <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
      <div className="flex min-w-0 flex-wrap items-center gap-1"><span className="text-sm font-medium">{title}</span><HelpHint term={title} what={t(option.extension ? 'providerFields.extensionHelp' : 'providerFields.fieldHelp')} /></div>
      {mode === 'custom' ? <Button size="sm" variant="ghost" disabled={disabled} onClick={raw}>{t('providerFields.custom')}</Button>
        : <Select<Mode> size="sm" hideLabel label={t('providerFields.ruleFor', { name: title })} value={mode} options={modes} disabled={disabled}
          onValueChange={(next) => {
            if (next === 'inherit') change(undefined);
            else if (next === 'block') change(false);
            else if (next === 'remove') change({ const: null, 'x-orch-replace': true });
            else if (next === 'fixed') change({ type: 'string', const: typeof rule.const === 'string' ? rule.const : '', 'x-orch-replace': true });
            else if (next === 'allow') change(suggestedRule(option));
          }} />}
    </div>
    <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-sm text-text-muted"><code className="wrap-anywhere">{option.path.join('.')}</code>
      {property.required && <span>{t('providerFields.required')}</span>}{property.state === 'absent' && property.closed && <span>{t('providerFields.closed')}</span>}
    </div>
    {mode === 'fixed' && <Input label={t('providerFields.fixedFor', { name: title })} hideLabel value={typeof rule.const === 'string' ? rule.const : ''} disabled={disabled}
      onChange={(event) => change({ ...rule, const: event.target.value })} />}
    {mode === 'allow' && option.kind === 'boolean' && <Select<'either' | 'true' | 'false'> size="sm" label={t('providerFields.allowed')} value={JSON.stringify(rule.enum) === '[true]' ? 'true' : JSON.stringify(rule.enum) === '[false]' ? 'false' : 'either'} disabled={disabled}
      options={[{ value: 'either', label: t('providerFields.either') }, { value: 'true', label: t('settings.editor.words.on') }, { value: 'false', label: t('settings.editor.words.off') }]}
      onValueChange={(value) => { const next = { ...rule }; if (value === 'either') Reflect.deleteProperty(next, 'enum'); else next.enum = [value === 'true']; change(next); }} />}
    {mode === 'allow' && (option.kind === 'string' || option.kind === 'strings') && <>
      <Switch presentation="compact" label={t(option.kind === 'strings' ? 'providerFields.limitItems' : 'providerFields.limitValues')} checked={option.kind === 'strings' ? Array.isArray(record(rule.items).enum) : Array.isArray(rule.enum)} disabled={disabled}
        onCheckedChange={(on) => {
          if (on) updateValues((option.values ?? values).join(', '));
          else { const next = { ...rule }; if (option.kind === 'strings') { const items = { ...record(rule.items) }; Reflect.deleteProperty(items, 'enum'); next.items = items; } else Reflect.deleteProperty(next, 'enum'); change(next); }
        }} />
      {(option.kind === 'strings' ? Array.isArray(record(rule.items).enum) : Array.isArray(rule.enum)) && <Input label={t('providerFields.valuesFor', { name: title })} value={draft.basis === basis ? draft.text : values.join(', ')} disabled={disabled} onChange={(event) => updateValues(event.target.value)} />}
      {option.kind === 'string' && values.length > 0 && <Select<string> size="sm" label={t('providerFields.defaultFor', { name: title })} value={typeof rule.default === 'string' ? rule.default : '\u0000none'} disabled={disabled}
        options={[{ value: '\u0000none', label: t('providerFields.noDefault') }, ...[...new Set([...values, ...(typeof rule.default === 'string' ? [rule.default] : [])])].map((value) => ({ value, label: value }))]} onValueChange={updateDefault} />}
    </>}
  </div>;
}

export interface OptionSchemaEditorProps {
  value: string;
  onChange: (value: string) => void;
  protocols: readonly string[];
  disabled?: boolean;
  error?: string;
}
export function OptionSchemaEditor({ value, onChange, protocols, disabled = false, error }: OptionSchemaEditorProps) {
  const { t } = useTranslation();
  const [selected, setSelected] = useState(protocols[0] ?? '');
  const [rawOpen, setRawOpen] = useState(false);
  const textarea = useRef<HTMLTextAreaElement>(null);
  let document: Record<string, unknown> | undefined;
  try { const parsed: unknown = value.trim() === '' ? {} : JSON.parse(value); if (parsed !== null && typeof parsed === 'object' && !Array.isArray(parsed)) document = parsed as Record<string, unknown>; } catch { /* The raw draft remains intact. */ }
  const available = [...new Set([...protocols, ...Object.keys(document ?? {}).filter((name) => name !== 'native')])];
  const protocol = available.includes(selected) ? selected : available[0] ?? '';
  const fields = documentedOptions(protocol);
  const openRaw = () => { setRawOpen(true); requestAnimationFrame(() => textarea.current?.focus()); };
  return <div className="flex min-w-0 flex-col gap-3" data-option-schema-editor>
    {available.length > 0 && <Select<string> size="sm" label={t('providerFields.protocol')} value={protocol} options={available.map((format) => ({ value: format, label: format }))} disabled={disabled} onValueChange={setSelected} />}
    {document === undefined ? <InlineAlert tone="danger">{t('providerOptions.invalidSchema')}</InlineAlert> : fields.length === 0 ? null
      : <div className="flex min-w-0 flex-col">{fields.map((option) => <OptionRow key={`${protocol}:${option.path.join('.')}`} option={option} property={optionProperty(document, protocol, option.path)} disabled={disabled}
        raw={openRaw} change={(rule) => { const next = setOptionProperty(document, protocol, option.path, rule); if (next.ok) onChange(JSON.stringify(next.value, null, 2)); else openRaw(); }} />)}</div>}
    <details open={rawOpen || document === undefined || error !== undefined || fields.length === 0} onToggle={(event) => setRawOpen(event.currentTarget.open)}>
      <summary className="cursor-default text-sm font-medium text-accent-text">{t('providerFields.raw')}</summary>
      <Textarea ref={textarea} label={t('providerFields.raw')} className="mt-2 font-mono" rows={6} value={value} disabled={disabled} error={error} onChange={(event) => onChange(event.target.value)} />
    </details>
  </div>;
}
