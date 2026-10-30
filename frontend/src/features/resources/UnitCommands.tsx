/** Named literal programs and arguments run after a unit starts, with bounded deadlines. */
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { Button } from '@/ui/Button';
import { Input } from '@/ui/Input';
import { Textarea } from '@/ui/Textarea';

export interface UnitCommand { name: string; command: string[]; timeout_secs: number }
export const record = (value: unknown): Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {};

export function commandOf(name: string, value: unknown): UnitCommand | null {
  const row = record(value);
  if (!Array.isArray(row.command) || !row.command.every((part): part is string => typeof part === 'string')) return null;
  return { name, command: row.command, timeout_secs: typeof row.timeout_secs === 'number' ? row.timeout_secs : 300 };
}

export function commandMap(value: unknown): UnitCommand[] {
  return Object.entries(record(value)).flatMap(([name, value]) => { const command = commandOf(name, value); return command === null ? [] : [command]; }).sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
}

export function templateCommands(spec: unknown): UnitCommand[] {
  const value = record(spec);
  const commands = Array.isArray(value.after_up) ? value.after_up.flatMap((entry) => {
    const row = record(entry);
    const command = typeof row.name === 'string' ? commandOf(row.name, row) : null;
    return command === null ? [] : [command];
  }) : [];
  if (!commands.some((command) => command.name === 'init')) {
    const init = commandOf('init', value.init);
    if (init !== null) commands.push(init);
  }
  return commands.sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
}

export function validCommands(commands: readonly UnitCommand[]): boolean {
  return new Set(commands.map((command) => command.name)).size === commands.length && commands.every((command) =>
    command.name.trim() !== '' && !Array.from(command.name).some((character) => character.charCodeAt(0) < 32 || character.charCodeAt(0) >= 127 && character.charCodeAt(0) <= 159) && command.command[0] !== undefined && command.command[0].trim() !== '' && !command.command.some((argument) => argument.includes('\0')) &&
    Number.isInteger(command.timeout_secs) && command.timeout_secs >= 1 && command.timeout_secs <= 3600);
}

/** One argument per line keeps spaces, quoting characters and paths literal. */
export function CommandListEditor({ value, onChange, disabled = false }: { value: readonly UnitCommand[]; onChange: (value: UnitCommand[]) => void; disabled?: boolean }) {
  const { t } = useTranslation();
  const replace = (index: number, change: Partial<UnitCommand>) => onChange(value.map((command, position) => position === index ? { ...command, ...change } : command));
  return <div className="flex flex-col gap-3">
    <p className="text-sm text-text-muted">{t('unitConfiguration.literalArguments')}</p>
    {value.map((command, index) => <fieldset key={index} className="flex flex-col gap-2 rounded-lg border border-border p-3" disabled={disabled}>
      <legend className="px-1 text-sm">{t('unitConfiguration.commandNumber', { number: index + 1 })}</legend>
      <Input label={t('unitConfiguration.commandName')} value={command.name} onChange={(event) => replace(index, { name: event.target.value })} />
      <Input label={t('unitConfiguration.program')} value={command.command[0] ?? ''} onChange={(event) => replace(index, { command: [event.target.value, ...command.command.slice(1)] })} />
      <Textarea label={t('unitConfiguration.arguments')} value={command.command.slice(1).join('\n')} onChange={(event) => replace(index, { command: [command.command[0] ?? '', ...(event.target.value === '' ? [] : event.target.value.split('\n'))] })} rows={3} />
      <Input label={t('unitConfiguration.timeout')} type="number" min={1} max={3600} value={command.timeout_secs} onChange={(event) => replace(index, { timeout_secs: Number(event.target.value) })} />
      <Button size="sm" variant="ghost" disabled={disabled} onClick={() => onChange(value.filter((_, position) => position !== index))}>{t('unitConfiguration.remove')}</Button>
    </fieldset>)}
    <Button size="sm" disabled={disabled} onClick={() => onChange([...value, { name: '', command: [''], timeout_secs: 300 }])}>{t('unitConfiguration.addCommand')}</Button>
  </div>;
}

export function CommandList({ commands, label }: { commands: readonly UnitCommand[]; label: string }) {
  const { t } = useTranslation();
  return <div><h3 className="text-sm font-medium">{label}</h3>{commands.length === 0 ? <EmptyState compact title={t('unitConfiguration.noCommands')} /> :
    <ul className="flex flex-col gap-2">{commands.map((command) => <li key={command.name} className="rounded-md border border-border p-2">
      <span className="font-medium">{command.name}</span><code className="block break-all text-xs">{command.command.map((part) => part === '' || /\s|["'\\]/u.test(part) ? JSON.stringify(part) : part).join(' ')}</code>
      <span className="text-xs text-text-muted">{t('unitConfiguration.commandTime', { seconds: command.timeout_secs })}</span>
    </li>)}</ul>}</div>;
}

/** The programs declared by a provider template; their executable and arguments remain literal. */
export function TemplatePrograms({ spec }: { spec: unknown }) {
  const { t } = useTranslation();
  const plugins = record(spec).plugins;
  const programs = Array.isArray(plugins) ? plugins.map(record) : [];
  return <section aria-label={t('unitConfiguration.programs')} className="flex flex-col gap-2">
    <h3 className="text-sm font-medium">{t('unitConfiguration.programs')}</h3>
    <p className="text-sm text-text-muted">{t('unitConfiguration.templateProgramsReadOnly')}</p>
    {programs.length === 0 ? <EmptyState compact title={t('unitConfiguration.noPrograms')} /> : <ul className="flex flex-col gap-2">
      {programs.map((program, index) => <li key={index} className="rounded-md border border-border p-2">
        <span className="font-medium">{typeof program.name === 'string' ? program.name : ''}</span>
        <span className="ml-2 text-sm text-text-muted">{program.kind === 'provider' ? t('unitConfiguration.kinds.provider') : program.kind === 'translator' ? t('unitConfiguration.kinds.translator') : program.kind === 'adapter' ? t('unitConfiguration.kinds.adapter') : program.kind === 'executor' ? t('unitConfiguration.kinds.executor') : ''}</span>
        <code className="block break-all text-xs">{JSON.stringify(program.command)}</code>
      </li>)}
    </ul>}
  </section>;
}
