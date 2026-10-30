/** People: who holds which role in a place, who holds one from above, and what each role does there as things stand; a role as a name and a count, with its people on request.
 *
 * A table of the people and groups who hold a role in this place, with a menu to change the role or
 * its end, or to take it away; the role a member holds at their home org is changed, never removed
 * here. Those who hold a role in a place above are summed up, one quiet line for each place and role,
 * and open their people on request. "Add people…" gives a role. A sentence under the table says what each
 * role does here as things stand, read from the defaults, and leads to the tab where that is changed.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { ChevronRight, Ellipsis, Pencil, Plus, Trash2, Users } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';
import type { TFunction } from 'i18next';

import { type AccessTarget, changeRole, changeRoleDetails, defaultsQuery, giveRole, permissionsQuery, rolesQuery, roleSubject, takeRole } from '@/api/access';
import { usePlaceLabel } from '@/features/navigation/placeLabels';
import { meQuery } from '@/api/auth';
import { ApiError } from '@/api/client';
import { nodeQuery } from '@/api/nodes';
import type { RoleHeld, Role, Subject } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { denial, holds } from '@/lib/permissions';
import { Badge } from '@/ui/Badge';
import { actionName } from '@/ui/actions';
import { Button } from '@/ui/Button';
import { Combobox } from '@/ui/Combobox';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { DateField } from '@/ui/DateField';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { RelativeTime } from '@/ui/RelativeTime';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { Table, type Column } from '@/ui/Table';
import { toast } from '@/ui/Toast';
import { TracePerson, useTraceEntry } from '@/features/trace/Person';
import { scopeOf, type TraceScope } from '@/features/trace/params';

import { type Candidate, useCandidates, useTargetOrg } from './candidates';
import { choiceWords } from './AddHolders';
import { choiceOfRole } from './gives';
import { HolderView, shortDate } from './holders';
import { holderKey, indexCatalogue, linesOf, ROLE_ORDER } from './model';

function subjectName(subject: Subject, t: TFunction, label: ReturnType<typeof usePlaceLabel>): string {
  if (subject.kind === 'user' && subject.user !== null && subject.user !== undefined) return subject.user.display_name;
  if (subject.kind === 'group' && subject.group !== null && subject.group !== undefined) return `${subject.group.name}, ${label({id:subject.group.org_id,name:subject.group.place ?? null}) ?? subject.group.place ?? t('access.place.hidden')}`;
  if (subject.kind === 'role') return subject.role === 'viewer' ? t('access.holder.everyoneIn', { place: label(subject.at) ?? t('access.place.hidden') }) : t('access.holder.roleOf', { roles: t(`access.holder.roles.${subject.role ?? 'viewer'}`), place: label(subject.at) ?? t('access.place.hidden') });
  return t('access.holder.membersOf', { place: label(subject.org) ?? t('access.place.hidden') });
}

/** A person, a group or everybody of an org, as the line of a table. */
export function SubjectView({ subject, scope, allowed = false }: { subject: Subject; scope?: TraceScope | undefined; allowed?: boolean }) {
  const { t } = useTranslation();
  const label = usePlaceLabel();
  if (subject.kind === 'role') return <span>{subjectName(subject,t,label)}</span>;
  if (subject.kind === 'user' && subject.user !== null && subject.user !== undefined) return scope === undefined ? <span>{subject.user.display_name}</span> : <TracePerson user={subject.user} scope={scope} allowed={allowed} />;
  if (subject.kind === 'group' && subject.group !== null && subject.group !== undefined) {
    return (
      <span className="inline-flex min-w-0 items-center gap-2">
        <Users className="text-text-muted" aria-hidden />
        <span className="font-medium wrap-anywhere text-text">{subjectName(subject,t,label)}</span>
        <span className="text-text-muted">· {subject.group.member_count}</span>
      </span>
    );
  }
  return <span className="font-medium wrap-anywhere text-text">{subjectName(subject,t,label)}</span>;
}

/** What each role does here as things stand, read from what the place gives to what is made inside it. */
function WhatRolesDo({ place, onDefaults }: { place: AccessTarget; onDefaults?: (() => void) | undefined }) {
  const { t } = useTranslation();
  const catalogue = useQuery(permissionsQuery);
  const defaults = useQuery(defaultsQuery(place.id));
  if (catalogue.data === undefined || defaults.data === undefined) return null;
  const index = indexCatalogue(catalogue.data);
  const kinds = place.kind === 'org' ? (['project', 'topic', 'thread'] as const) : place.kind === 'project' ? (['topic', 'thread'] as const) : (['thread'] as const);
  const sentences = kinds.flatMap((kind) => {
    // The page of a thing of that kind, made of the rows of its kind and of the work that runs in a managed environment.
    const rows = defaults.data.rows.filter((row) => index.defs.get(row.permission)?.on.includes(kind) === true && (row.profile === 'any' || row.profile === 'managed'));
    const lines = linesOf({ ...defaults.data, rows }, index, kind);
    const said = (['view', 'interact', 'manage'] as const).flatMap((line) => {
      const roles = lines[line].filter((holder) => holder.holder.kind === 'role' && holder.holder.at === null);
      return roles.map((holder) =>
        holder.holder.kind === 'role'
          ? t('access.roles.does', { who: holder.holder.role === 'viewer' ? t('access.roles.everyone') : t(`access.holder.roles.${holder.holder.role}`), what: t(`access.roles.verb.${kind}.${line}`) })
          : '',
      );
    });
    return said.length === 0 ? [] : [{ kind, said }];
  });
  return (
    <div className="flex flex-col gap-2 text-sm text-text-muted">
      {sentences.map(({ kind, said }) => (
        <p key={kind}>
          <span className="font-medium text-text">{t(`access.roles.kind.${kind}`)}</span> {said.join('; ')}.
        </p>
      ))}
      {onDefaults !== undefined && <div>
        <Button size="sm" variant="ghost" onClick={onDefaults}>
          {actionName('defaults')}
        </Button>
      </div>}
    </div>
  );
}

/** Roles held in a place above: one quiet line for each place and role, opening the people on request. */
function Above({ rows, scope, allowed }: { rows: readonly RoleHeld[]; scope?: TraceScope | undefined; allowed: boolean }) {
  const { t } = useTranslation();
  const label = usePlaceLabel();
  const [open, setOpen] = useState<string | null>(null);
  const groups = new Map<string, { node: RoleHeld['node']; role: Role; rows: RoleHeld[] }>();
  for (const row of rows) {
    const key = `${row.node.id}:${row.role}`;
    const known = groups.get(key);
    if (known === undefined) groups.set(key, { node: row.node, role: row.role, rows: [row] });
    else known.rows.push(row);
  }
  if (groups.size === 0) return null;
  const people = (list: RoleHeld[]) => list.reduce((sum, row) => sum + (row.subject.kind === 'group' ? (row.subject.group?.member_count ?? 1) : 1), 0);
  return (
    <ul aria-label={t('access.roles.above')} className="flex flex-col divide-y divide-border rounded-lg border border-border bg-bg-subtle">
      {[...groups.entries()].map(([key, group]) => (
        <li key={key} className="px-3 py-2 text-sm text-text-muted">
          <button
            type="button"
            aria-expanded={open === key}
            className="flex w-full cursor-default items-center gap-2 text-left hover:text-text"
            onClick={() => setOpen(open === key ? null : key)}
          >
            <ChevronRight className={open === key ? 'rotate-90' : undefined} size={16} aria-hidden />
            <span>{t('access.roles.aboveLine', { roles: t(`access.holder.roles.${group.role}`), place: label(group.node) ?? t('access.place.hidden'), count: people(group.rows) })}</span>
          </button>
          {open === key && (
            <ul className="mt-2 flex flex-col gap-1 pl-6">
              {group.rows.map((row) => (
                <li key={row.id}>
                  <SubjectView subject={row.subject} scope={scope} allowed={allowed} />
                </li>
              ))}
            </ul>
          )}
        </li>
      ))}
    </ul>
  );
}

/** Gives a role in the place to a person, a group or the members of an org. */
function AddPeople({ place, open, onClose, givable }: { place: AccessTarget; open: boolean; onClose: () => void; givable: readonly Role[] }) {
  const { t } = useTranslation();
  const orgId = useTargetOrg(place);
  const [query, setQuery] = useState('');
  const [chosen, setChosen] = useState<Candidate | null>(null);
  const [wanted, setRole] = useState<Role>('viewer');
  // The role begins as the lowest the caller may give, and stays one they may.
  const role: Role = givable.includes(wanted) ? wanted : (ROLE_ORDER.find((entry) => givable.includes(entry)) ?? wanted);
  const [until, setUntil] = useState('');
  const candidates = useCandidates(query, orgId, { kinds: ['user', 'group', 'role', 'members'], thing: place.name, placeId: place.id });

  const give = useMutation({
    mutationFn: () => {
      if (chosen === null) throw new Error('nobody was chosen');
      const subject = roleSubject(chosen.input, place.id);
      if (subject === null) throw new Error('that holder cannot hold a role');
      return giveRole(place.id, {
        subject,
        role,
        ...(until === '' ? {} : { expires_at: new Date(`${until}T23:59:59`).toISOString() }),
      });
    },
    onSuccess: () => {
      toast({ tone: 'success', title: t('access.roles.given', { name: chosen?.name ?? '', role: t(`roles.${role}`) }) });
      onClose();
    },
  });
  const exists = give.error instanceof ApiError && give.error.is('conflict.exists');

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !give.isPending && onClose()}
      size="sm"
      title={t('access.roles.addTitle', { name: place.name })}
      description={t('access.roles.addLead')}
      dismissible={!give.isPending}
      footer={
        <>
          <Button disabled={give.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" pending={give.isPending} blocked={chosen === null} onClick={() => give.mutate()}>
            {t('access.roles.give')}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4 py-2">
        {candidates.placePicker}
        <Combobox
          label={t('access.roles.who')}
          placeholder={t('access.roles.personPlaceholder')}
          query={query}
          onQueryChange={(next) => {
            setQuery(next);
            setChosen(null);
          }}
          minChars={0}
          loading={candidates.loading}
          empty={t('access.candidates.none')}
          groups={candidates.groups}
          onSelect={(option) => {
            const found = candidates.byValue.get(option.value);
            if (found === undefined) return;
            setChosen(found);
            setQuery(found.name);
          }}
        />
        <Select<Role> label={t('access.roles.role')} value={role} onValueChange={setRole} options={ROLE_ORDER.map((value) => ({ value, ...choiceOption(t, `accessRole.${value}`), disabled: !givable.includes(value), ...(givable.includes(value) ? {} : { description: `${choiceOption(t, `accessRole.${value}`).description} ${t('access.gives.notYours')}` }) }))} />
        <DateField label={t('access.add.until')} optional value={until} onValueChange={setUntil} />
        {give.error !== null && (exists ? <p className="text-sm text-danger-text">{t('access.roles.exists')}</p> : <ErrorAlert error={give.error} />)}
      </div>
    </Dialog>
  );
}

/** The one line that says which roles the reader may give here and why not the rest: "You are a Developer of Web: you can give up to Can manage of its work, and not Admin." */
function GivesLine({ place, givable }: { place: string; givable: readonly Role[] }) {
  const { t } = useTranslation();
  const top = [...ROLE_ORDER].reverse().find((role) => givable.includes(role));
  if (top === undefined || top === 'admin') return null;
  const above = ROLE_ORDER[ROLE_ORDER.indexOf(top) + 1] ?? 'admin';
  const choice = choiceWords(t, choiceOfRole(top), 'topic');
  return <p className="text-sm text-text-muted">{t(`access.gives.youAre.${top}`, { place, choice, above: t(`roles.${above}`) })}</p>;
}

export function Roles({ place, onDefaults }: { place: AccessTarget; onDefaults?: (() => void) | undefined }) {
  const { t } = useTranslation();
  const label = usePlaceLabel();
  const roles = useQuery(rolesQuery(place.id));
  const node = useQuery(nodeQuery(place.id));
  const me = useQuery(meQuery).data;
  const locale = useAppearance((state) => state.locale);
  const [adding, setAdding] = useState(false);
  const [taking, setTaking] = useState<RoleHeld | null>(null);
  const [ending, setEnding] = useState<RoleHeld | null>(null);
  const [end, setEnd] = useState('');
  const may = holds(node.data?.access, 'access.manage');
  const scope = node.data === undefined ? undefined : scopeOf(node.data);
  const allowed = holds(node.data?.access, 'audit.view');
  const traceEntry = useTraceEntry(scope ?? null, allowed);
  const reason = denial(t, 'access.manage', place.name, node.data?.owner);
  // Whoever shares without managing the page gives the roles up to their own, and takes back, and changes, the ones they gave.
  const givable: readonly Role[] = roles.data?.can_give ?? [];
  const mayTouch = (row: RoleHeld) => may || (me?.id === row.created_by.id && givable.includes(row.role));

  const change = useMutation({
    mutationFn: ({ row, role }: { row: RoleHeld; role: Role }) => changeRole(row.id, role, row.revision),
  });
  const take = useMutation({
    mutationFn: (row: RoleHeld) => takeRole(row.id),
    onSuccess: () => {
      setTaking(null);
    },
  });
  const finish = useMutation({
    mutationFn: ({ row, expires }: { row: RoleHeld; expires: string | null }) => changeRoleDetails(row.id, row.revision, { expires_at: expires }),
    onSuccess: () => {
      setEnding(null);
    },
  });

  if (roles.isPending) return <SkeletonRows rows={4} />;
  if (roles.isError) return <ErrorState compact error={roles.error} retrying={roles.isRefetching} onRetry={() => void roles.refetch()} />;
  const own = roles.data.own;

  const columns: Column<RoleHeld>[] = [
    {
      id: 'who',
      grow: true,
      header: t('access.roles.who'),
      cell: (row) => (
        <span className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
          <SubjectView subject={row.subject} scope={scope} allowed={allowed} />
          {row.domain && <Badge>{t('access.roles.domain')}</Badge>}
          {row.outside && <Badge tone="warning">{t('access.holder.outside')}</Badge>}
        </span>
      ),
    },
    {
      id: 'role',
      header: t('access.roles.role'),
      cell: (row) =>
        mayTouch(row) ? (
          <Select<Role>
            label={t('access.roles.changeFor', { name: subjectName(row.subject, t,label) })}
            hideLabel
            size="sm"
            value={row.role}
            disabled={change.isPending && change.variables.row.id === row.id}
            onValueChange={(next) => next !== row.role && change.mutate({ row, role: next })}
            options={ROLE_ORDER.map((value) => ({ value, ...choiceOption(t, `accessRole.${value}`), disabled: !givable.includes(value) && value !== row.role }))}
            fieldClassName="w-[9rem]"
          />
        ) : (
          t(`roles.${row.role}`)
        ),
    },
    {
      id: 'gave',
      header: t('access.roles.gave'),
      from: 'md',
      cell: (row) => <span className="text-text-muted">{t('access.roles.gaveBy', { name: row.created_by.display_name, date: shortDate(locale, row.created_at) })}</span>,
    },
    {
      id: 'until',
      header: t('access.roles.until'),
      from: 'sm',
      cell: (row) => (row.expires_at === null ? <span className="text-text-muted">—</span> : <RelativeTime at={row.expires_at} className="text-text-muted" />),
    },
    {
      id: 'menu',
      header: t('access.roles.actions'),
      hideHeader: true,
      align: 'end',
      cell: (row) => {
        const name = subjectName(row.subject, t,label);
        const entries: MenuEntry[] = [
          ...(row.subject.kind === 'user' && row.subject.user ? traceEntry(row.subject.user) : []),
          {
            id: 'end',
            label: t('access.roles.setEnd'),
            icon: <Pencil aria-hidden />,
            disabled: mayTouch(row) ? false : reason,
            onSelect: () => {
              setEnd(row.expires_at === null ? '' : row.expires_at.slice(0, 10));
              setEnding(row);
            },
          },
          { type: 'separator', id: 'separator' },
          {
            id: 'take',
            label: t('access.roles.take'),
            icon: <Trash2 aria-hidden />,
            danger: true,
            disabled: !mayTouch(row) ? reason : row.domain ? t('access.roles.domainKept') : false,
            onSelect: () => setTaking(row),
          },
        ];
        return <Menu label={t('access.roles.menuOf', { name })} align="end" entries={entries} trigger={<IconButton size="sm" label={t('access.roles.menuOf', { name })} icon={<Ellipsis aria-hidden />} />} />;
      },
    },
  ];

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-sm text-text-muted">{t('access.roles.lead', { name: place.name })}</p>
        <Button size="sm" icon={<Plus aria-hidden />} blocked={givable.length === 0} onClick={() => setAdding(true)} title={givable.length > 0 ? undefined : reason}>
          {t('access.roles.add')}
        </Button>
      </div>
      {roles.data.owners.length > 0 && <section aria-label={t('access.owners.label')}>
        <ul className="flex flex-wrap gap-2">{roles.data.owners.map((holder) => <li key={holderKey(holder)}>
          <HolderView holder={holder} target={place} note={holder.kind === 'user' && holder.user.id === node.data?.owner.id ? t('access.roster.primaryOwner') : t('access.roster.owner')} />
        </li>)}</ul>
      </section>}
      {own.length === 0 ? (
        roles.data.owners.length === 0 ? <EmptyState compact icon={<Users aria-hidden />} title={t('access.roles.none')} description={t('access.roles.noneMore')} /> : null
      ) : (
        <Table label={t('access.roles.table', { name: place.name })} columns={columns} rows={own} rowKey={(row) => row.id} />
      )}
      {change.error !== null && <ErrorAlert error={change.error} />}
      <GivesLine place={place.name} givable={givable} />
      <Above rows={roles.data.above} scope={scope} allowed={allowed} />
      <WhatRolesDo place={place} onDefaults={onDefaults} />

      {adding && <AddPeople place={place} open onClose={() => setAdding(false)} givable={givable} />}
      <ConfirmDialog
        open={taking !== null}
        onOpenChange={(next) => !next && setTaking(null)}
        title={t('access.roles.takeTitle', { name: taking === null ? '' : subjectName(taking.subject, t,label) })}
        confirmLabel={t('access.roles.takeConfirm', { name: taking === null ? '' : subjectName(taking.subject, t,label) })}
        danger
        pending={take.isPending}
        error={take.error ?? undefined}
        onConfirm={() => taking !== null && take.mutate(taking)}
      >
        {t('access.roles.takeBody', { role: taking === null ? '' : t(`roles.${taking.role}`), place: place.name })}
      </ConfirmDialog>
      <Dialog
        open={ending !== null}
        onOpenChange={(next) => !next && !finish.isPending && setEnding(null)}
        size="sm"
        title={t('access.roles.endTitle', { name: ending === null ? '' : subjectName(ending.subject, t,label) })}
        footer={
          <>
            <Button disabled={finish.isPending} onClick={() => setEnding(null)}>
              {t('common.cancel')}
            </Button>
            <Button
              variant="primary"
              pending={finish.isPending}
              onClick={() => ending !== null && finish.mutate({ row: ending, expires: end === '' ? null : new Date(`${end}T23:59:59`).toISOString() })}
            >
              {t('access.roles.endSave')}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-3 py-2">
          <DateField label={t('access.roles.until')} value={end} onValueChange={setEnd} hint={t('access.roles.endHint')} />
          {finish.error !== null && <ErrorAlert error={finish.error} />}
        </div>
      </Dialog>
    </div>
  );
}
