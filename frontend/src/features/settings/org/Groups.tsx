/** Groups and membership.
 *
 * The groups an org keeps, which a page or a role can name instead of each person: a name, a
 * description, and who is in it. Whoever manages the members makes, renames and removes a group and
 * changes who is in it; a group's people must be members of the root org. A group taken away takes
 * with it the roles it held and the entries that named it, and the dialog says so before it asks.
 */
import { useCollectionSearch } from '@/lib/collectionSearch';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { ScrollRegion } from '@/ui/ScrollRegion';
import { linkTo } from '@/lib/links';
import { copyText } from '@/ui/clipboard';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { usePageContext } from '@/features/workbench/pageContext';
import { useInfiniteQuery, useMutation, useQuery } from '@tanstack/react-query';
import { Ellipsis, Pencil, Plus, Trash2, UserPlus, UsersRound, X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import {
  addGroupHolder,
  createGroup,
  deleteGroup,
  groupHoldersPageQuery,
  groupsQuery,
  groupsPageQuery,
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
import { useDebounced } from '@/lib/debounce';
import { FilterBar } from '@/ui/FilterBar';
import { Pagination } from '@/ui/Pagination';

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
  const search = useCollectionSearch("group-holders:" + group.id);
  const people = useInfiniteQuery(groupHoldersPageQuery(group.id, search.q));
  const holders = people.data?.pages.flatMap((page) => page.items) ?? [];
  const metadata = useQuery(groupsQuery(group.org_id, false));
  const current = metadata.data?.find((row) => row.id === group.id) ?? group;
  const refresh = () => { void metadata.refetch(); void people.refetch(); };
  const [query, setQuery] = useState('');
  const found = useCandidates(query, org.org_id, { kinds: ['user', 'group', 'role', 'members'], thing: org.name, placeId: org.id });
  const inside = new Set(holders.map((holder) => holderKey(holder)));
  const add = useMutation({
    mutationFn: (member: Candidate) => addGroupHolder(group.id, member.input, current.revision),
    onSuccess: (_none, person) => {
      setQuery('');
      refresh();
      toast({ tone: 'success', title: t('members.groups.added', { name: person.name, group: current.name }) });
    },
    onError: refresh,
  });
  const take = useMutation({ mutationFn: (holder: Holder) => removeGroupHolder(group.id, inputOf(holder)), onSuccess: refresh, onError: refresh });
  const groups = found.groups.map((section) => ({ ...section, options: section.options.map((option) => {
    const candidate = found.byValue.get(option.value);
    return { ...option, disabled: candidate !== undefined && inside.has(holderKey(candidate.input)) ? t('members.groups.alreadyIn') : option.disabled };
  }) }));
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && onClose()}
      title={t('members.groups.membersTitle', { name: current.name })}
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
        <CollectionSearch name={t("members.groups.membersTitle", { name: current.name })} {...search} />
        {add.error !== null && <ErrorAlert error={add.error} />}
        {people.isPending ? (
          <SkeletonRows read={people} rows={3} />
        ) : people.isError && holders.length === 0 ? (
          <ErrorState compact error={people.error} retrying={people.isRefetching} onRetry={() => void people.refetch()} />
        ) : holders.length === 0 ? (
          <EmptyState compact title={t('members.groups.empty')} />
        ) : (
          <ScrollRegion kind="table" label={t("members.groups.membersTitle", { name: current.name })} className="max-h-96 overflow-y-auto"><ul className={`${boxClasses} divide-y divide-border`}>
            {holders.map((person) => (
              <li key={holderKey(person)} className="flex items-center gap-3 px-4 py-2">
                <span className="min-w-0 flex-1">
                  <HolderView holder={person} target={{ kind: 'org', id: org.id, name: org.name, orgId: org.org_id }} />
                </span>
                {mayManage && <IconButton size="sm" label={t('members.groups.take', { name: holderName(t, person, org.name) })} icon={<X aria-hidden />} onClick={() => take.mutate(person)} />}
              </li>
            ))}
          </ul></ScrollRegion>
        )}
        <Pagination hasMore={people.hasNextPage} loading={people.isFetchingNextPage} error={people.isFetchNextPageError ? people.error : undefined} onMore={() => void people.fetchNextPage()} />
        {take.error !== null && <ErrorAlert error={take.error} />}
      </div>
    </Dialog>
  );
}

export function Groups({ org }: { org: Org }) {
  const { t } = useTranslation();
  const [under, setUnder] = useState(false);
  const [text, setText] = useState('');
  const q = useDebounced(text.trim());
  const list = useInfiniteQuery(groupsPageQuery(org.id, under, q));
  const rows = list.data?.pages.flatMap((page) => page.items) ?? [];
  const [editing, setEditing] = useState<Group | 'new' | null>(null);
  const selected = usePageContext()?.target.search.group;
  const linked = useQuery({ ...groupsQuery(org.id, true), enabled: typeof selected === 'string' && !rows.some((row) => row.id === selected) });
  const selectionScope = JSON.stringify([org.id, selected]);
  const [inspection, setInspection] = useState<{ scope: string; group: Group | null } | null>(null);
  const looking = inspection?.scope === selectionScope ? inspection.group : typeof selected === 'string' ? rows.find((group) => group.id === selected) ?? linked.data?.find((group) => group.id === selected) ?? null : null;
  const setLooking = (group: Group | null) => setInspection({ scope: selectionScope, group });
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
  const entriesFor = (group: Group): MenuEntry[] => [
    { id: 'copy-link', action: 'copyLink', label: t('actions.copyLink.label'), onSelect: () => void copyText(linkTo({ kind: 'group', id: group.id, orgId: group.org_id }), t('common.linkCopied')) },
  ];
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
          <Menu label={t('resources.act.menuOf', { name: group.name })} entries={entriesFor(group)} trigger={<IconButton size="sm" label={t('resources.act.menuOf', { name: group.name })} icon={<Ellipsis aria-hidden />} />} />
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
      <FilterBar search={<Input type="search" hideLabel label={t('members.groups.find')} placeholder={t('ui.combobox.search')} value={text} onChange={(event) => setText(event.target.value)} />}>
        {org.org_count > 0 && <Switch label={t('members.groups.under')} checked={under} onCheckedChange={setUnder} className="items-center gap-3" />}
      </FilterBar>
      {list.isPending ? (
        <SkeletonRows read={list} rows={3} />
      ) : list.isError && rows.length === 0 ? (
        <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
      ) : rows.length === 0 ? (
        <EmptyState compact icon={<UserPlus aria-hidden />} title={t('members.groups.none', { name: org.name })} description={may ? t('members.groups.noneAbout') : undefined} />
      ) : (
        <Table key={`${under}:${q}`} label={t('members.groups.table', { name: org.name })} columns={columns} rows={rows} virtual rowKey={(group) => group.id} rowMenu={{ label: (group) => t('resources.act.menuOf', { name: group.name }), entries: entriesFor }} />
      )}
      <Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} />
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
