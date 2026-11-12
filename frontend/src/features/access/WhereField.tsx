/** The place an entry lets work run in: the environments, hosts and machines it names, as one compact control, and the names of them wherever a refusal says where.
 *
 * An entry that names nothing lets the work run anywhere, so the control says "Anywhere" and a name is added from the
 * product's own resource picker. A host or a machine that is named stands for every environment on it. A name whose
 * object can no longer be read is shown as gone and is kept or removed like any other: it never blocks saving.
 */
import { useQuery } from '@tanstack/react-query';
import { Box, Laptop, Server, X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { environmentsQuery } from '@/api/environments';
import { hostsQuery, machinesQuery } from '@/api/resources';
import { resourceChoice, ResourceChip } from '@/features/resource-facts/ResourceFacts';
import type { WhereName } from '@/generated/api-types';
import { cx } from '@/ui/cx';
import { IconButton } from '@/ui/IconButton';
import { ResourcePicker } from '@/ui/ResourcePicker';
import type { SelectOption } from '@/ui/Select';

type Kind = WhereName['kind'];

/** The kinds of place a name may be of, in the order the picker lists them. */
const KINDS: readonly Kind[] = ['environment', 'host', 'machine'];
const ICONS = { environment: Box, host: Server, machine: Laptop } as const;

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** The names a stored value holds: the members that read as a kind and an identifier, in the order they were written. */
export function whereNames(value: unknown): WhereName[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item: unknown) => isRecord(item) && typeof item.id === 'string' && (item.kind === 'environment' || item.kind === 'host' || item.kind === 'machine') ? [{ kind: item.kind, id: item.id } satisfies WhereName] : []);
}

const sameName = (one: WhereName, other: WhereName) => one.kind === other.kind && one.id === other.id;
const encode = (name: WhereName) => `${name.kind}:${name.id}`;

/** The environments, hosts and machines the person sees, read when a name is shown or the picker is open. */
function useKnown(wanted: readonly WhereName[], open: boolean) {
  const need = (kind: Kind) => open || wanted.some((name) => name.kind === kind);
  const environments = useQuery({ ...environmentsQuery(null), enabled: need('environment') });
  const hosts = useQuery({ ...hostsQuery(null), enabled: need('host') });
  const machines = useQuery({ ...machinesQuery, enabled: need('machine') });
  const names = new Map<string, string>();
  for (const row of environments.data ?? []) names.set(encode({ kind: 'environment', id: row.id }), row.name);
  for (const row of hosts.data ?? []) names.set(encode({ kind: 'host', id: row.id }), row.name);
  for (const row of machines.data ?? []) names.set(encode({ kind: 'machine', id: row.id }), row.name);
  // A list that was not asked for is idle and pending; it neither loads nor fails.
  const asked = [environments, hosts, machines].filter((query) => query.fetchStatus !== 'idle' || query.status !== 'pending');
  return { names, environments, hosts, machines, loading: asked.some((query) => query.isPending), failed: asked.some((query) => query.isError) };
}
type Known = ReturnType<typeof useKnown>;

/** One name: its object's own chip where it can be read, otherwise gone. */
function PlaceChip({ name, known }: { name: WhereName; known: Known }) {
  const { t } = useTranslation();
  const Icon = ICONS[name.kind];
  const label = known.names.get(encode(name));
  if (label !== undefined && name.kind === 'environment') {
    const row = known.environments.data?.find((each) => each.id === name.id);
    if (row !== undefined) return <ResourceChip subject={{ kind: 'environment', environment: row }} hint={false} />;
  }
  if (label !== undefined && name.kind === 'host') {
    const row = known.hosts.data?.find((each) => each.id === name.id);
    if (row !== undefined) return <ResourceChip subject={{ kind: 'host', host: row }} hint={false} />;
  }
  const text = label ?? (known.loading ? t('common.loading') : known.failed ? t('settings.resourceChoice.unavailable') : t('access.options.gone'));
  return (
    <span data-where-name={name.kind} data-gone={label === undefined && !known.loading && !known.failed ? true : undefined} className={cx('resource-chip inline-flex min-w-0 max-w-full items-center gap-1 rounded-md border bg-surface px-1.5 py-0.5 text-sm', label === undefined ? 'border-border text-text-muted' : 'border-control-border')}>
      <Icon aria-hidden className="size-4 shrink-0 text-text-muted" />
      <span className="min-w-0 truncate">{text}</span>
    </span>
  );
}

export interface WhereFieldProps {
  /** What the field is called. */
  label: string;
  /** The names the entry holds now; none means anywhere. */
  value: readonly WhereName[];
  onChange: (next: WhereName[]) => void;
  disabled?: boolean;
}

/** The control of the entry's options: "Anywhere" or the names it holds, each removable, and a picker that adds one. */
export function WhereField({ label, value, onChange, disabled = false }: WhereFieldProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const known = useKnown(value, open);
  const offered: SelectOption<string>[] = KINDS.flatMap((kind): SelectOption<string>[] => {
    const taken = (id: string) => value.some((name) => sameName(name, { kind, id }));
    const group = t(`access.options.whereKinds.${kind}`);
    if (kind === 'environment') return (known.environments.data ?? []).filter((row) => !taken(row.id)).map((row): SelectOption<string> => ({ value: encode({ kind, id: row.id }), name: row.name, group, ...resourceChoice({ kind: 'environment', environment: row }) }));
    if (kind === 'host') return (known.hosts.data ?? []).filter((row) => !taken(row.id)).map((row): SelectOption<string> => ({ value: encode({ kind, id: row.id }), name: row.name, group, ...resourceChoice({ kind: 'host', host: row }) }));
    return (known.machines.data ?? []).filter((row) => !taken(row.id)).map((row): SelectOption<string> => ({ value: encode({ kind, id: row.id }), name: row.name, group, resource: { kind: 'machine' as const, id: row.id, data: row } }));
  });
  const add = (chosen: string) => {
    const [kind, ...rest] = chosen.split(':');
    const id = rest.join(':');
    if ((kind === 'environment' || kind === 'host' || kind === 'machine') && id !== '') onChange([...value, { kind, id }]);
  };
  return (
    <fieldset className="flex min-w-0 flex-col gap-2" data-where-field>
      <legend className="text-sm font-medium text-text">{label}</legend>
      {value.length === 0 ? (
        <p className="text-sm text-text-muted">{t('access.options.anywhere')}</p>
      ) : (
        <ul aria-label={label} className="flex min-w-0 flex-col gap-1.5">
          {value.map((name) => {
            const shown = known.names.get(encode(name)) ?? t('access.options.gone');
            return (
              <li key={encode(name)} className="flex min-w-0 items-center gap-2">
                <span className="min-w-0 flex-1"><PlaceChip name={name} known={known} /></span>
                <IconButton size="sm" label={t('access.options.whereRemove', { name: shown })} icon={<X aria-hidden />} disabled={disabled} onClick={() => onChange(value.filter((each) => !sameName(each, name)))} />
              </li>
            );
          })}
        </ul>
      )}
      <ResourcePicker<string> pickerKey="access.EntryOptions.where" size="sm" label={t('access.options.whereAdd', { what: label })} hideLabel value="" open={open} onOpenChange={setOpen} disabled={disabled}
        placeholder={t(open && known.loading ? 'common.loading' : 'access.options.addName')} options={offered} onValueChange={add} />
    </fieldset>
  );
}

/** The places an entry or a refusal names, by name; one that cannot be read is said to be gone, or by `unreadable` where the reader may only not see it. */
export function WhereNames({ names, unreadable }: { names: readonly WhereName[]; unreadable?: string }) {
  const { t } = useTranslation();
  const known = useKnown(names, false);
  return <>{names.map((name, index) => <span key={encode(name)}>{index > 0 && ', '}<span className="font-medium">{known.names.get(encode(name)) ?? (known.loading ? t('common.loading') : unreadable ?? t('access.options.gone'))}</span></span>)}</>;
}
