/** Every root org with its owners and its size, each of which a system admin enters as an Admin of it; naming an owner where none is left; and the two limits of each: whether it takes members, and whether it holds orgs inside it.
 *
 * Each root org is a row: its name, which opens it like any link; its first owner and how many there
 * are; how many people, projects and orgs it holds; and its two limits as switches, which only this
 * screen changes. Switching a limit off removes nobody and no org: the row says how many stay. A
 * personal org has both off as shipped. An org whose owners are gone can be given one here. Those that
 * were deleted and are not purged yet stand under the list, each with Restore.
 */
import { useInfiniteQuery, useMutation, useQuery } from '@tanstack/react-query';
import { ArchiveRestore, Building, ExternalLink, Search, UserRoundPlus } from 'lucide-react';
import { Link } from '@tanstack/react-router';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { adminOrgsQuery, nameOwner, orgPeopleQuery, restoreOrg, setOrgLimits } from '@/api/admin';
import { useSearchBox } from '@/features/admin/useSearchBox';
import type { SectionProps } from '@/features/admin/sections';
import type { Member, Org } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { useDebounced } from '@/lib/debounce';
import { formatNumber } from '@/lib/format';
import { Section } from '@/shell/Page';
import { Avatar } from '@/ui/Avatar';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { StatusTag } from '@/features/admin/StatusTag';
import { Button, buttonClasses } from '@/ui/Button';
import { Combobox } from '@/ui/Combobox';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { FilterBar } from '@/ui/FilterBar';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { type Column, Table } from '@/ui/Table';
import { toast } from '@/ui/Toast';
import { UserChip } from '@/ui/UserChip';
import { identityColor } from '@/lib/grapheme';

/** How many of the people that match what is typed the list shows: past that, a few more letters narrow it. */
const LIST_SHOWN = 8;

/** The words the screen says of one of the two limits: the switch, and what stays when it is off. */
type Limit = 'closed' | 'flat';

/** One limit as a switch that says what is so, and under it how many stay where it is off. */
function LimitSwitch({ org, limit }: { org: Org; limit: Limit }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const off = org[limit];
  const change = useMutation({
    mutationFn: (allowed: boolean) => setOrgLimits(org.id, { [limit]: !allowed }),
  });
  const holds = limit === 'closed' ? org.member_count : org.org_count;
  return (
    <div className="flex flex-col gap-1">
      <Switch
        label={t(`admin.orgs.limits.${limit}.label`)}
        // What was asked is shown while it is on its way: the switch answers at once, and goes back if the system refuses.
        checked={change.isPending ? change.variables : !off}
        disabled={change.isPending}
        onCheckedChange={(next) => change.mutate(next)}
        className="items-center gap-3"
      />
      {change.isPending && (
        <span role="status" className="text-sm text-text-muted">
          {t('admin.settingRow.saving')}
        </span>
      )}
      {off && holds > 0 && org.org_kind !== 'personal' && !change.isPending && <span className="text-sm text-text-muted">{t(`admin.orgs.limits.${limit}.stays`, { count: holds, number: formatNumber(holds, locale) })}</span>}
      {change.error !== null && <ErrorAlert error={change.error} />}
    </div>
  );
}

/** Names one of an org's people an owner, for an org that has none left. */
function OwnerDialog({ org, onClose }: { org: Org; onClose: () => void }) {
  const { t } = useTranslation();
  const [typed, setTyped] = useState('');
  const asked = useDebounced(typed);
  const people = useQuery(orgPeopleQuery(org.id, asked));
  const [chosen, setChosen] = useState<Member | null>(null);

  const name = useMutation({
    mutationFn: (member: Member) => nameOwner(org.id, member.user.id),
    onSuccess: (_node, member) => {
      toast({ tone: 'success', title: t('admin.orgs.owner.named', { name: member.user.display_name, org: org.name }) });
      onClose();
    },
  });

  return (
    <Dialog
      open
      onOpenChange={(open) => !open && !name.isPending && onClose()}
      title={t('admin.orgs.owner.title', { org: org.name })}
      description={t('admin.orgs.owner.lead')}
      dismissible={!name.isPending}
      footer={
        <>
          <Button disabled={name.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" pending={name.isPending} disabled={chosen === null} onClick={() => chosen !== null && name.mutate(chosen)}>
            {t('admin.orgs.owner.confirm')}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4 py-2">
        <Combobox<string>
          label={t('admin.orgs.owner.person')}
          hint={t('admin.orgs.owner.hint')}
          placeholder={t('admin.orgs.owner.placeholder')}
          minChars={1}
          query={typed}
          onQueryChange={setTyped}
          loading={people.isFetching}
          empty={t('admin.orgs.owner.nobody')}
          groups={[
            {
              id: 'people',
              label: t('admin.orgs.owner.people'),
              options: (people.data?.items ?? []).slice(0, LIST_SHOWN).map((member) => ({
                value: member.user.id,
                text: `${member.user.display_name} @${member.user.username}`,
                label: <UserChip user={member.user} size={24} />,
                description: `@${member.user.username}`,
              })),
            },
          ]}
          onSelect={(option) => {
            setChosen(people.data?.items.find((member) => member.user.id === option.value) ?? null);
            setTyped('');
          }}
        />
        {chosen !== null && (
          <p className="flex items-center gap-2 text-base text-text">
            {t('admin.orgs.owner.chosen')} <UserChip user={chosen.user} size={24} showUsername />
          </p>
        )}
        {name.error !== null && <ErrorAlert error={name.error} />}
      </div>
    </Dialog>
  );
}

export function Organizations({ search, onSearch }: SectionProps) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const [typed, setTyped] = useSearchBox(search.q, (q) => onSearch({ q }));
  // Both kinds by default, the teams first; the filter narrows to one, and is kept in the address.
  const filter = { q: search.q ?? '', deleted: false, ...(search.orgKind === undefined ? {} : { orgKind: search.orgKind }) };
  const list = useInfiniteQuery(adminOrgsQuery(filter));
  const gone = useInfiniteQuery(adminOrgsQuery({ q: search.q ?? '', deleted: true }));
  const orgs = list.data?.pages.flatMap((page) => page.items) ?? [];
  const deleted = gone.data?.pages.flatMap((page) => page.items) ?? [];
  const [naming, setNaming] = useState<Org | null>(null);

  const restore = useMutation({
    mutationFn: (org: Org) => restoreOrg(org.id),
    onSuccess: (_node, org) => {
      toast({ tone: 'success', title: t('admin.orgs.restored', { name: org.name }) });
    },
  });

  const size = (org: Org) => (
    <span className="flex flex-col text-sm whitespace-nowrap text-text-muted">
      <span>{t('admin.orgs.size.people', { count: org.member_count, number: formatNumber(org.member_count, locale) })}</span>
      <span>{t('admin.orgs.size.projects', { count: org.project_count, number: formatNumber(org.project_count, locale) })}</span>
      <span>{t('admin.orgs.size.orgs', { count: org.org_count, number: formatNumber(org.org_count, locale) })}</span>
    </span>
  );

  const nameCell = (org: Org) => (
    <span className="flex min-w-0 items-center gap-2">
      <Avatar text={Array.from(org.name)[0]?.toUpperCase() ?? ''} color={identityColor(org.id)} kind={org.org_kind === 'personal' ? 'human' : 'org'} size={28} />
      <span className="flex min-w-0 flex-col">
        <span className="flex flex-wrap items-center gap-2">
          <span className="font-medium wrap-anywhere text-text">{org.name}</span>
          {org.org_kind === 'personal' && <StatusTag>{t('admin.orgs.personal')}</StatusTag>}
          {org.archived && <StatusTag tone="warning">{t('admin.orgs.archived')}</StatusTag>}
        </span>
        <code className="text-sm text-text-subtle">{org.address}</code>
      </span>
    </span>
  );

  const columns: Column<Org>[] = [
    { id: 'org', header: t('admin.orgs.org'), cell: nameCell },
    {
      id: 'owner',
      header: t('admin.orgs.owners'),
      from: 'sm',
      cell: (org) =>
        org.owners === 0 ? (
          <span className="flex flex-col items-start gap-1">
            <StatusTag tone="warning">{t('admin.orgs.noOwners')}</StatusTag>
            <Button size="sm" icon={<UserRoundPlus aria-hidden />} onClick={() => setNaming(org)}>
              {t('admin.orgs.owner.open')}
            </Button>
          </span>
        ) : (
          <span className="flex min-w-0 flex-col">
            <UserChip user={org.owner} size={24} />
            {org.owners > 1 && <span className="text-sm text-text-muted">{t('admin.orgs.andOthers', { count: org.owners - 1, number: formatNumber(org.owners - 1, locale) })}</span>}
          </span>
        ),
    },
    { id: 'size', header: t('admin.orgs.size.title'), from: 'md', cell: size },
    {
      id: 'limits',
      header: t('admin.orgs.limits.title'),
      cell: (org) => (
        <div className="flex min-w-[13rem] flex-col gap-3">
          <LimitSwitch org={org} limit="closed" />
          <LimitSwitch org={org} limit="flat" />
        </div>
      ),
    },
    {
      id: 'open',
      header: t('admin.orgs.open'),
      hideHeader: true,
      align: 'end',
      cell: (org) => (
        <Link to="/org/$slug" params={{ slug: org.address }} className={buttonClasses({ size: 'sm' })}>
          <ExternalLink aria-hidden />
          {t('admin.orgs.open')}
        </Link>
      ),
    },
  ];

  const deletedColumns: Column<Org>[] = [
    { id: 'org', header: t('admin.orgs.org'), cell: nameCell },
    { id: 'owner', header: t('admin.orgs.owners'), from: 'sm', cell: (org) => <UserChip user={org.owner} size={24} /> },
    { id: 'size', header: t('admin.orgs.size.title'), from: 'md', cell: size },
    { id: 'when', header: t('admin.orgs.lastChanged'), from: 'md', cell: (org) => <RelativeTime at={org.updated_at} className="text-text-muted" /> },
    {
      id: 'restore',
      header: t('admin.orgs.restore'),
      hideHeader: true,
      align: 'end',
      cell: (org) => (
        <Button size="sm" icon={<ArchiveRestore aria-hidden />} pending={restore.isPending && restore.variables.id === org.id} blocked={restore.isPending && restore.variables.id !== org.id} onClick={() => restore.mutate(org)}>
          {t('admin.orgs.restore')}
        </Button>
      ),
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-col gap-4">
        <FilterBar search={<Input
          label={t('admin.orgs.search')}
          hideLabel
          type="search"
          placeholder={t('admin.orgs.searchPlaceholder')}
          value={typed}
          onChange={(event) => setTyped(event.target.value)}
          fieldClassName="min-w-0 sm:max-w-(--container-form)"
          autoComplete="off"
        />}>
        <SegmentedControl
          label={t('admin.orgs.kind.label')}
          value={search.orgKind ?? 'all'}
          onValueChange={(next) => onSearch({ orgKind: next === 'all' ? undefined : next })}
          segments={[
            { value: 'all', ...choiceOption(t, 'orgKind.all') },
            { value: 'team', ...choiceOption(t, 'orgKind.team'), name: t('admin.orgs.teamOrgs') },
            { value: 'personal', ...choiceOption(t, 'orgKind.personal') },
          ]}
        />
        </FilterBar>
        {list.isPending ? (
          <SkeletonRows rows={4} />
        ) : list.isError && orgs.length === 0 ? (
          <ErrorState error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
        ) : orgs.length === 0 ? (
          <EmptyState compact icon={filter.q === '' ? <Building aria-hidden /> : <Search aria-hidden />} title={filter.q === '' ? t('admin.orgs.none') : t('admin.orgs.noMatch')} />
        ) : (
          <div className="flex flex-col gap-2">
            <Table label={t('admin.sections.orgs.title')} columns={columns} rows={orgs} rowKey={(org) => org.id} />
            <Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} />
          </div>
        )}
      </div>

      {deleted.length > 0 && (
        <Section id="admin-orgs-deleted" title={t('admin.orgs.deletedTitle')} lead={t('admin.orgs.deletedLead')}>
          <Table label={t('admin.orgs.deletedTitle')} columns={deletedColumns} rows={deleted} rowKey={(org) => org.id} />
          {restore.error !== null && <ErrorAlert error={restore.error} />}
          <Pagination hasMore={gone.hasNextPage} loading={gone.isFetchingNextPage} error={gone.isFetchNextPageError ? gone.error : undefined} onMore={() => void gone.fetchNextPage()} />
        </Section>
      )}

      {naming !== null && <OwnerDialog org={naming} onClose={() => setNaming(null)} />}
    </div>
  );
}

