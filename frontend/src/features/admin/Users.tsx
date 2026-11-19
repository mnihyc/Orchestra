/** Users: creation with what their own org starts as, suspending and resuming, admin status, reset links that may switch a second step off, and a name that strangers' wrong tries have closed.
 *
 * Everybody who has an account, by name, a page at a time, with a search and the two filters that
 * are asked for: who may sign in, and who administers the system. The acts on a person are in the menu
 * of their row; each says what it does before it does it. A row whose name strangers' wrong tries have
 * locked says which proof is locked until when, and what to tell the person, who is not locked out of
 * the browsers they signed in from. A person's name opens their card, and what they did is one step
 * from it and from their row: the system's trail of that person.
 */
import { useInfiniteQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { Ellipsis, KeyRound, Pause, Play, Search, ShieldUser, TicketPlus, TicketX, Link2, UserPlus, UserRoundX, Users as UsersIcon } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { giveOrgInvitation, takeBackOrgInvitation, type UserFilter, usersQuery } from '@/api/admin';
import { useSystemSettings } from '@/features/admin/SettingRows';
import { CreateUserDialog, ResetLinkDialog, type Standing, StandingDialog } from '@/features/admin/UserDialogs';
import { useSearchBox } from '@/features/admin/useSearchBox';
import type { SectionProps } from '@/features/admin/sections';
import type { AdminUser, AdminUserLock } from '@/generated/api-types';
import { TracePerson, useTraceEntry } from '@/features/trace/Person';
import { useAppearance } from '@/lib/appearance';
import { formatTimeOfDay } from '@/lib/time';
import { StatusTag } from '@/features/admin/StatusTag';
import { InviteDialog } from '@/features/org/Invite';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { FilterBar } from '@/ui/FilterBar';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { SkeletonRows } from '@/ui/Skeleton';
import { type Column, Table } from '@/ui/Table';
import { toast } from '@/ui/Toast';
import { useErrorText } from '@/ui/errorText';

type StatusChoice = 'all' | 'active' | 'suspended';

/**
 * Giving a person an invitation to make one team org, and taking back one they have not used. The
 * entries are offered while the system lets only invited people make team orgs, which is when an
 * invitation counts; the list is read again after either, so that the row says what the server holds.
 */
function useOrgInvitations() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const words = useErrorText();
  const { find } = useSystemSettings();
  const counts = find('orgs.created_by')?.value === 'invited';
  const change = useMutation({
    mutationFn: ({ user, give }: { user: AdminUser; give: boolean }) => (give ? giveOrgInvitation(user.id) : takeBackOrgInvitation(user.id)),
    onSuccess: (_, { user, give }) => {
      toast({ tone: 'success', title: t(give ? 'admin.users.orgInvitation.given' : 'admin.users.orgInvitation.takenBack', { name: user.display_name }) });
    },
    onError: (error) => toast({ tone: 'danger', title: t('admin.users.orgInvitation.failed'), description: words(error).message }),
    onSettled: (_data, error) => { if (error !== null) void queryClient.invalidateQueries({ queryKey: ['admin', 'users'] }); },
  });
  const entries = (user: AdminUser): MenuEntry[] => {
    if (!counts || user.status === 'suspended' || user.is_system_admin) return [];
    const held = user.org_invited_at !== null;
    return [
      held
        ? {
            id: 'org-invitation',
            label: t('admin.users.orgInvitation.takeBack'),
            description: t('admin.users.orgInvitation.takeBackAbout'),
            icon: <TicketX aria-hidden />,
            disabled: change.isPending ? t('admin.users.orgInvitation.pending') : false,
            onSelect: () => change.mutate({ user, give: false }),
          }
        : {
            id: 'org-invitation',
            label: t('admin.users.orgInvitation.give'),
            description: t('admin.users.orgInvitation.giveAbout'),
            icon: <TicketPlus aria-hidden />,
            disabled: change.isPending ? t('admin.users.orgInvitation.pending') : false,
            onSelect: () => change.mutate({ user, give: true }),
          },
    ];
  };
  return { counts, entries };
}

/** The line under a person's name for each proof strangers' wrong tries have closed at it. */
function Locks({ locks }: { locks: readonly AdminUserLock[] }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  if (locks.length === 0) return null;
  return (
    <span className="flex flex-col gap-1 text-sm text-text-muted">
      {locks.map((lock) => (
        <span key={lock.way} className="flex items-start gap-1">
          <KeyRound aria-hidden className="mt-0.5 shrink-0" />
          <span>{t(`admin.users.locked.${lock.way}`, { time: formatTimeOfDay(lock.until, { locale, timeZone }) })}</span>
        </span>
      ))}
      <span>{t('admin.users.locked.advice')}</span>
    </span>
  );
}

export function Users({ search, onSearch }: SectionProps) {
  const { t } = useTranslation();
  const [typed, setTyped] = useSearchBox(search.q, (q) => onSearch({ q }));
  const status: StatusChoice = search.status ?? 'all';
  const filter: UserFilter = { q: search.q ?? '', status: status === 'all' ? '' : status, admins: search.admins === true };
  const list = useInfiniteQuery(usersQuery(filter));
  const people = list.data?.pages.flatMap((page) => page.items) ?? [];
  const [making, setMaking] = useState(false);
  const [inviting, setInviting] = useState(false);
  const [linking, setLinking] = useState<AdminUser | null>(null);
  const [changing, setChanging] = useState<{ user: AdminUser; act: Standing } | null>(null);
  const filtered = filter.q !== '' || filter.status !== '' || filter.admins;
  const scope = { type: 'system' as const, name: t('trace.systemTrail') };
  const traced = useTraceEntry(scope, true);
  const invitations = useOrgInvitations();

  const columns: Column<AdminUser>[] = [
    {
      id: 'person',
      header: t('admin.users.person'),
      cell: (user) => (
        <span className="flex min-w-0 flex-col gap-1">
          <span className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
            <TracePerson user={user} scope={scope} allowed showUsername />
            {user.is_system_admin && <StatusTag tone="accent">{t('admin.users.admin')}</StatusTag>}
            {user.status === 'suspended' && <StatusTag tone="warning">{t('admin.users.suspended')}</StatusTag>}
            {invitations.counts && user.org_invited_at !== null && (
              <StatusTag tone="neutral">{t('admin.users.orgInvitation.held')}</StatusTag>
            )}
          </span>
          <Locks locks={user.locked} />
        </span>
      ),
    },
    {
      id: 'address',
      header: t('admin.users.address'),
      from: 'sm',
      cell: (user) =>
        user.email === null ? (
          <span className="text-text-subtle">{t('admin.users.noAddress')}</span>
        ) : (
          <span className="flex min-w-[14rem] flex-col">
            <span className="wrap-anywhere">{user.email}</span>
            {!user.email_verified && <span className="text-sm text-text-muted">{t('admin.users.unconfirmed')}</span>}
          </span>
        ),
    },
    {
      id: 'second',
      header: t('admin.users.secondStep'),
      from: 'lg',
      className: 'whitespace-nowrap',
      cell: (user) =>
        user.totp_enabled || user.email_code_second_step ? (
          <span>{[user.totp_enabled ? t('admin.users.totp') : null, user.email_code_second_step ? t('admin.users.mailedCode') : null].filter(Boolean).join(', ')}</span>
        ) : (
          <span className="text-text-subtle">{t('admin.users.noSecondStep')}</span>
        ),
    },
    {
      id: 'seen',
      header: t('admin.users.lastSeen'),
      from: 'md',
      className: 'whitespace-nowrap',
      cell: (user) => (user.last_seen_at === null ? <span className="text-text-subtle">{t('admin.users.neverSeen')}</span> : <RelativeTime at={user.last_seen_at} className="text-text-muted" />),
    },
    {
      id: 'menu',
      header: t('admin.users.actions'),
      hideHeader: true,
      align: 'end',
      cell: (user) => (
        <Menu
          label={t('admin.users.menuOf', { name: user.display_name })}
          align="end"
          entries={[
            ...traced(user),
            { id: 'link', label: t('admin.users.link.entry'), icon: <KeyRound aria-hidden />, onSelect: () => setLinking(user) },
            {
              id: 'admin',
              label: user.is_system_admin ? t('admin.users.revoke.entry') : t('admin.users.grant.entry'),
              icon: user.is_system_admin ? <UserRoundX aria-hidden /> : <ShieldUser aria-hidden />,
              onSelect: () => setChanging({ user, act: user.is_system_admin ? 'revoke' : 'grant' }),
            },
            ...invitations.entries(user),
            { type: 'separator', id: 'apart' },
            user.status === 'suspended'
              ? { id: 'resume', label: t('admin.users.resume.entry'), icon: <Play aria-hidden />, onSelect: () => setChanging({ user, act: 'resume' }) }
              : { id: 'suspend', label: t('admin.users.suspend.entry'), icon: <Pause aria-hidden />, danger: true, onSelect: () => setChanging({ user, act: 'suspend' }) },
          ]}
          trigger={<IconButton size="sm" label={t('admin.users.menuOf', { name: user.display_name })} icon={<Ellipsis aria-hidden />} />}
        />
      ),
    },
  ];

  return (
    <div className="flex flex-col gap-4">
      <FilterBar search={<Input
          label={t('admin.users.search')}
          hideLabel
          type="search"
          placeholder={t('admin.users.searchPlaceholder')}
          value={typed}
          onChange={(event) => setTyped(event.target.value)}
          fieldClassName="min-w-0 grow basis-[14rem] sm:max-w-(--container-form)"
          autoComplete="off"
        />} action={<div className="flex flex-wrap gap-2">
          <Button variant="primary" icon={<Link2 aria-hidden />} onClick={() => setInviting(true)}>{t('admin.users.invite.open')}</Button>
          <Button icon={<UserPlus aria-hidden />} onClick={() => setMaking(true)}>{t('admin.users.create.open')}</Button>
        </div>}>
        <SegmentedControl
          label={t('admin.users.status')}
          value={status}
          onValueChange={(next) => onSearch({ status: next === 'all' ? undefined : next })}
          segments={[
            { value: 'all', name: t('choiceCatalog.accountState.all.name') },
            { value: 'active', name: t('choiceCatalog.accountState.active.name') },
            { value: 'suspended', name: t('choiceCatalog.accountState.suspended.name') },
          ]}
        />
        <Checkbox label={t('admin.users.adminsOnly')} checked={search.admins === true} onCheckedChange={(next) => onSearch({ admins: next ? true : undefined })} className="items-center" />
      </FilterBar>

      {list.isPending ? (
        <SkeletonRows rows={5} />
      ) : list.isError && people.length === 0 ? (
        <ErrorState error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
      ) : people.length === 0 ? (
        <EmptyState compact icon={filtered ? <Search aria-hidden /> : <UsersIcon aria-hidden />} title={filtered ? t('admin.users.noMatch') : t('admin.users.none')} />
      ) : (
        <div className="flex flex-col gap-2">
          <Table label={t('admin.sections.users.title')} columns={columns} rows={people} rowKey={(user) => user.id} rowClassName={(user) => (user.status === 'suspended' ? 'bg-bg-subtle' : undefined)} />
          <Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} />
        </div>
      )}

      {inviting && <InviteDialog target={{ kind: 'system' }} open onClose={() => setInviting(false)} />}
      {making && <CreateUserDialog open onClose={() => setMaking(false)} />}
      {linking !== null && <ResetLinkDialog user={linking} onClose={() => setLinking(null)} />}
      {changing !== null && <StandingDialog user={changing.user} act={changing.act} onClose={() => setChanging(null)} />}
    </div>
  );
}
