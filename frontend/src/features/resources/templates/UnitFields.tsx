/** Template commands and registered programs stay visible and are edited as literal argument lists. */
import { useTranslation } from 'react-i18next';
import { CommandListEditor, record, templateCommands } from '@/features/resources/UnitCommands';
import { useTemplateChoiceWords } from './choiceWords';
import { Button } from '@/ui/Button';
import { FieldForm } from '@/ui/FieldForm';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { Textarea } from '@/ui/Textarea';

const KINDS = ['provider', 'translator', 'adapter', 'executor'] as const;
const strings = (value: unknown): string[] => Array.isArray(value) ? value.filter((part): part is string => typeof part === 'string') : [];
const variables = (value: unknown): Record<string, string> => Object.fromEntries(Object.entries(record(value)).filter((entry): entry is [string, string] => typeof entry[1] === 'string'));

export function TemplateUnitFields({ spec, kind, disabled, onChange }: { spec: Record<string, unknown>; kind: 'provider' | 'environment'; disabled: boolean; onChange: (spec: Record<string, unknown>) => void }) {
  const { t } = useTranslation();
  const choiceWords=useTemplateChoiceWords();
  const kindOptions=KINDS.map((value)=>({value,...choiceWords(['plugins','0','kind'],value)}));
  const restartOptions=['on-failure','always','never'].map((value)=>({value,...choiceWords(['plugins','0','restart'],value)}));
  const programs = Array.isArray(spec.plugins) ? spec.plugins.map((value) => {
    const row = record(value);
    return { kind: typeof row.kind === 'string' ? row.kind : 'provider', name: typeof row.name === 'string' ? row.name : '', command: strings(row.command), env: variables(row.env), restart: typeof row.restart === 'string' ? row.restart : 'on-failure' };
  }) : [];
  const patch = (index: number, values: Partial<(typeof programs)[number]>) => onChange({ ...spec, plugins: programs.map((program, position) => position === index ? { ...program, ...values } : program) });
  return <div className="flex flex-col gap-6">
    <section aria-label={t('unitConfiguration.commands')} className="flex flex-col gap-2">
      <h3 className="font-medium">{t('unitConfiguration.commands')}</h3>
      <p className="text-sm text-text-muted">{t('unitConfiguration.templateCommandHelp')}</p>
      {spec.init !== undefined && spec.init !== null && <p className="text-sm text-text-muted">{t('unitConfiguration.initSuperseded')}</p>}
      <CommandListEditor value={templateCommands(spec)} disabled={disabled} onChange={(commands) => {
        const next: Record<string, unknown> = { ...spec, after_up: commands }; delete next.init; onChange(next);
      }} />
    </section>
    {kind === 'provider' && <section aria-label={t('unitConfiguration.programs')} className="flex flex-col gap-3">
      <h3 className="font-medium">{t('unitConfiguration.programs')}</h3>
      <p className="text-sm text-text-muted">{t('unitConfiguration.programsHelp')}</p>
      {programs.map((program, index) => <fieldset key={index} disabled={disabled} className="flex flex-col gap-2 rounded-lg border border-border p-3">
        <legend className="px-1 text-sm">{t('unitConfiguration.programNumber', { number: index + 1 })}</legend>
        <Select label={t('unitConfiguration.programKind')} value={program.kind} onValueChange={(value) => patch(index, { kind: value })} options={kindOptions} disabled={disabled} />
        <Input label={t('unitConfiguration.programName')} value={program.name} onChange={(event) => patch(index, { name: event.target.value })} />
        <Input label={t('unitConfiguration.program')} value={program.command[0] ?? ''} onChange={(event) => patch(index, { command: [event.target.value, ...program.command.slice(1)] })} />
        <Textarea label={t('unitConfiguration.arguments')} value={program.command.slice(1).join('\n')} rows={3} onChange={(event) => patch(index, { command: [program.command[0] ?? '', ...(event.target.value === '' ? [] : event.target.value.split('\n'))] })} />
        <FieldForm fields={[{ name: 'env', label: t('unitConfiguration.programVariables'), type: 'map', required: false, help: null, choices: null, default: null, max_bytes: null }]} values={{ env: program.env }} onChange={(_, value) => patch(index, { env: variables(value) })} disabled={disabled} />
        <Select label={t('unitConfiguration.programRestart')} value={program.restart} onValueChange={(value) => patch(index, { restart: value })} options={restartOptions} disabled={disabled} />
        <Button size="sm" variant="ghost" disabled={disabled} onClick={() => onChange({ ...spec, plugins: programs.filter((_, position) => position !== index) })}>{t('unitConfiguration.remove')}</Button>
      </fieldset>)}
      <Button size="sm" disabled={disabled} onClick={() => onChange({ ...spec, plugins: [...programs, { kind: 'provider', name: '', command: [''], env: {}, restart: 'on-failure' }] })}>{t('unitConfiguration.addProgram')}</Button>
    </section>}
  </div>;
}
