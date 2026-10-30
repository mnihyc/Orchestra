/** Groups and membership.
 *
 * The groups an org keeps, which a page or a role can name instead of each person: a name, a
 * description, and who is in it. Whoever manages the members makes, renames and removes a group and
 * changes who is in it; a group's people must be members of the root org. A group taken away takes
 * with it the roles it held and the entries that named it, and the dialog says so before it asks.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Pencil, Plus, Trash2, UserPlus, UsersRound, X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import {
  addGroupHolder,
  createGroup,
  deleteGroup,
  groupHoldersQuery,
  groupsQuery,
  removeGroupHolder,
  updateGroup,
} from '@/api/directory';
import { type Candidate, useCandidates } from '@/features/access/candidates';
import { HolderView, holderName } from '@/features/access/holders';
import { holderKey, inputOf } from '@/features/access/model';
import { takes } from '@/features/settings/refusals';
import type { Group, Holder, Org } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { holds } from '@/lib/permissions';
import { boxClasses, Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { Combobox } from '@/ui/Combobox';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { PermissionGate } from '@/ui/PermissionGate';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { Table, type Column } from '@/ui/Table';
import { Textarea } from '@/ui/Textarea';
import { toast } from '@/ui/Toast';

/** The fewest characters typed before people are searched for: the server answers none for less. */

function GroupDialog({ org, group, onClose }: { org: Org; group: Group | null; onClose: () => void }) {
  const { t } = useTranslation();
  const actKey = useActKey();
  const [name, setName] = useState(group?.name ?? '');
  const [description, setDescription] = useState(group?.description ?? '');
  const [sent, setSent] = useState(false);
  const wrong = name.trim() === '' ? t('members.groups.nameMissing') : undefined;
  const save = useMutation({
    mutationFn: () => {
      const body = { name: name.trim(), description: description.trim() };
      return group === null ? createGroup(org.id, body, actKey(body)) : updateGroup(group.id, body);
    },
    onSuccess: () => {
      toast({ tone: 'success', title: group === null ? t('members.groups.made', { name: name.trim() }) : t('members.groups.changed', { name: name.trim() }) });
      onClose();
    },
  });
  const submit = () => {
    setSent(true);
    if (wrong === undefined && !save.isPending) save.mutate();
  };
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !save.isPending && onClose()}
      title={group === null ? t('members.groups.newTitle', { org: org.name }) : t('members.groups.editTitle', { name: group.name })}
      dismissible={!save.isPending}
      footer={
        <>
          <Button disabled={save.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" pending={save.isPending} onClick={submit}>
            {group === null ? t('members.groups.make') : t('settings.save')}
          </Button>
        </>
      }
    >
      <form
        noValidate
        className="flex flex-col gap-4 py-2"
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <Input label={t('members.groups.name')} value={name} error={sent ? wrong : undefined} onChange={(event) => setName(event.target.value)} autoComplete="off" />
        <Textarea label={t('members.groups.description')} optional rows={3} value={description} onChange={(event) => setDescription(event.target.value)} />
        {save.error !== null && <ErrorAlert error={save.error} />}
      </form>
    </Dialog>
  );
}

/** Who is in a group, with the field that adds a person of the org. */
function GroupMembers({ org, group, mayManage, onClose }: { org: Org; group: Group; mayManage: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const people = useQuery(groupHoldersQuery(group.id));
  const [query, setQuery] = useState('');
  const found = useCandidates(query, org.org_id, { kinds: ['user', 'group', 'role', 'members'], thing: org.name, placeId: org.id });
  const inside = new Set((people.data ?? []).map((holder) => holderKey(holder)));
  const add = useMutation({
    mutationFn: (member: Candidate) => addGroupHolder(group.id, member.input),
    onSuccess: (_none, person) => {
      setQuery('');
      toast({ tone: 'success', title: t('members.groups.added', { name: person.name, group: group.name }) });
    },
  });
  const take = useMutation({ mutationFn: (holder: Holder) => removeGroupHolder(group.id, inputOf(holder)) });
  const groups = found.groups.map((section) => ({ ...section, options: section.options.map((option) => {
    const candidate = found.byValue.get(option.value);
    return { ...option, disabled: candidate !== undefined && inside.has(holderKey(candidate.input)) ? t('members.groups.alreadyIn') : option.disabled };
  }) }));
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && onClose()}
      title={t('members.groups.membersTitle', { name: group.name })}
      description={t('members.groups.membersLead')}
      size="lg"
      footer={<Button onClick={onClose}>{t('common.close')}</Button>}
    >
      <div className="flex flex-col gap-4 py-2">
        {mayManage && <div className="flex flex-col gap-2">{found.placePicker}
          <Combobox
            label={t('members.groups.addPerson')}
            query={query}
            onQueryChange={setQuery}
            groups={groups}
            loading={found.loading || add.isPending}
            minChars={0}
            empty={t('access.candidates.none')}
            placeholder={t('members.groups.addPlaceholder')}
            onSelect={(option) => {
              const person = found.byValue.get(option.value);
              if (person !== undefined) add.mutate(person);
            }}
          />
        </div>}
        {add.error !== null && <ErrorAlert error={add.error} />}
        {people.isPending ? (
          <SkeletonRows rows={3} />
        ) : people.isError ? (
          <ErrorState compact error={people.error} retrying={people.isRefetching} onRetry={() => void people.refetch()} />
        ) : people.data.length === 0 ? (
          <EmptyState compact title={t('members.groups.empty')} />
        ) : (
          <ul className={`${boxClasses} divide-y divide-border`}>
            {people.data.map((person) => (
              <li key={holderKey(person)} className="flex items-center gap-3 px-4 py-2">
                <span className="min-w-0 flex-1">
                  <HolderView holder={person} target={{ kind: 'org', id: org.id, name: org.name, orgId: org.org_id }} />
                </span>
                {mayManage && <IconButton size="sm" label={t('members.groups.take', { name: holderName(t, person, org.name) })} icon={<X aria-hidden />} onClick={() => take.mutate(person)} />}
              </li>
            ))}
          </ul>
        )}
        {take.error !== null && <ErrorAlert error={take.error} />}
      </div>
    </Dialog>
  );
}

export function Groups({ org }: { org: Org }) {
  const { t } = useTranslation();
  const [under, setUnder] = useState(false);
  const list = useQuery(groupsQuery(org.id, under));
  const [editing, setEditing] = useState<Group | 'new' | null>(null);
  const [looking, setLooking] = useState<Group | null>(null);
  const [removing, setRemoving] = useState<Group | null>(null);
  const may = holds(org.access, 'members.manage');
  const reason = takes(t, 'members.manage', org.name, org.owner);
  const remove = useMutation({
    mutationFn: (group: Group) => deleteGroup(group.id),
    onSuccess: (_none, group) => {
      toast({ tone: 'success', title: t('members.groups.removed', { name: group.name }) });
      setRemoving(null);
    },
  });
  const columns: Column<Group>[] = [
    {
      id: 'name',
      header: t('members.groups.columns.name'),
      cell: (group) => (
        <span className="flex flex-col">
          <span className="font-medium wrap-anywhere text-text">{group.name}</span>
          {group.place !== null && group.place !== undefined && <span className="text-sm text-text-muted">{group.place}</span>}
          {group.description !== '' && <span className="text-sm wrap-anywhere text-text-muted">{group.description}</span>}
        </span>
      ),
    },
    { id: 'people', header: t('members.groups.columns.people'), from: 'sm', cell: (group) => t('members.groups.peopleCount', { count: group.member_count }) },
    {
      id: 'actions',
      header: t('members.groups.columns.actions'),
      hideHeader: true,
      align: 'end',
      cell: (group) => (
        <span className="flex items-center justify-end gap-1">
          <Button size="sm" variant="ghost" icon={<UsersRound aria-hidden />} onClick={() => setLooking(group)} aria-label={t('members.groups.openMembers', { name: group.name })}>
            {t('members.groups.membersButton')}
          </Button>
          {may && group.org_id === org.id && (
            <>
              <IconButton size="sm" label={t('members.groups.edit', { name: group.name })} icon={<Pencil aria-hidden />} onClick={() => setEditing(group)} />
              <IconButton size="sm" label={t('members.groups.remove', { name: group.name })} icon={<Trash2 aria-hidden />} onClick={() => setRemoving(group)} />
            </>
          )}
        </span>
      ),
    },
  ];
  return (
    <Section
      id="groups"
      title={t('settings.org.sectionNames.groups')}
      lead={t('members.groups.lead', { name: org.name })}
      action={
        <PermissionGate allowed={may} mode="block" reason={reason}>
          <Button size="sm" variant="primary" icon={<Plus aria-hidden />} onClick={() => setEditing('new')}>
            {t('members.groups.new')}
          </Button>
        </PermissionGate>
      }
    >
      {org.org_count > 0 && <Switch label={t('members.groups.under')} checked={under} onCheckedChange={setUnder} className="items-center gap-3" />}
      {list.isPending ? (
        <SkeletonRows rows={3} />
      ) : list.isError ? (
        <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
      ) : list.data.length === 0 ? (
        <EmptyState compact icon={<UserPlus aria-hidden />} title={t('members.groups.none', { name: org.name })} description={may ? t('members.groups.noneAbout') : undefined} />
      ) : (
        <Table label={t('members.groups.table', { name: org.name })} columns={columns} rows={list.data} rowKey={(group) => group.id} />
      )}
      {editing !== null && <GroupDialog org={org} group={editing === 'new' ? null : editing} onClose={() => setEditing(null)} />}
      {looking !== null && <GroupMembers org={org} group={looking} mayManage={may} onClose={() => setLooking(null)} />}
      <ConfirmDialog
        open={removing !== null}
        onOpenChange={(next) => !next && setRemoving(null)}
        title={t('members.groups.removeTitle', { name: removing?.name ?? '' })}
        confirmLabel={t('members.groups.removeConfirm', { name: removing?.name ?? '' })}
        danger
        pending={remove.isPending}
        error={remove.error ?? undefined}
        onConfirm={() => removing !== null && remove.mutate(removing)}
      >
        {t('members.groups.removeBody')}
      </ConfirmDialog>
    </Section>
  );
}
