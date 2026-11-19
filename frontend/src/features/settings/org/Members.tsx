/** The members of an org with their domain and role and the acts on each, and the invitations still good: the Members tab of the org, and the Members section of its settings; an org that takes no members says so.
 *
 * The org's home shows this list under its Members tab and the settings of the org under their Members section. It lists the members of the org it is opened on: everybody whose domain is that org or an org inside
 * it, or only those whose domain is the org itself. Whoever manages the members invites, in either
 * form and within the org's rules, changes a role, moves somebody to another org of the tree, holds
 * somebody out of the org and lifts that, and ends a membership; each of those says what it does
 * before it does it. What a reader may not do stays in the menu, greyed, with what it takes. An org
 * that takes no members says so, and what to do instead.
 */
import { useInfiniteQuery, useMutation, useQuery } from '@tanstack/react-query';
import { Ellipsis, MailPlus, MoveRight, Pause, Play, Search, ShieldCheck, UserMinus, Users } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useErrorText } from '@/ui/errorText';
import { choiceOption } from '@/i18n/choices';
import { useStore } from 'zustand';

import { changeRole, rolesQuery } from '@/api/access';
import { ApiError, client } from '@/api/client';
import { membersQuery, resumeMember, suspendMember, suspensionsQuery } from '@/api/directory';
import { invitationsQuery, revokeInvitation } from '@/api/users';
import { INVITED_ROLES, InviteDialog } from '@/features/org/Invite';
import { EndMembership } from '@/features/settings/org/EndMembership';
import { InvitationRules } from '@/features/settings/org/Invite';
import { MoveMember } from '@/features/settings/org/MoveMember';
import { Outsiders } from '@/features/settings/org/Outsiders';
import { takes } from '@/features/settings/refusals';
import type { Invitation, Member, Org, Role, Suspension } from '@/generated/api-types';
import { useDebounced } from '@/lib/debounce';
import { holds } from '@/lib/permissions';
import { useNow } from '@/lib/time';
import { Section } from '@/shell/Page';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { FilterBar } from '@/ui/FilterBar';
import { Menu } from '@/ui/Menu';
import { Pagination } from '@/ui/Pagination';
import { PermissionGate } from '@/ui/PermissionGate';
import { RelativeTime } from '@/ui/RelativeTime';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { Table, type Column } from '@/ui/Table';
import { Textarea } from '@/ui/Textarea';
import { toast } from '@/ui/Toast';
import { TracePerson, useTraceEntry } from '@/features/trace/Person';
import { scopeOf } from '@/features/trace/params';
import { InheritedMembers } from './InheritedMembers';

/** Whether an invitation can still be used: not taken back, not past its time, not used up. */
export function isPending(invitation: Invitation, now: number): boolean {
  return !invitation.revoked && invitation.uses < invitation.max_uses && Date.parse(invitation.expires_at) > now;
}

/** Changes the role a member holds at their domain. The row that holds it is found in the roles of that place. */
function ChangeRole({ member, onClose }: { member: Member; onClose: () => void }) {
  const { t } = useTranslation();
  const roles = useQuery(rolesQuery(member.domain.id));
  const row = roles.data?.own.find((held) => held.domain && held.subject.kind === 'user' && held.subject.id === member.user.id);
  const [role, setRole] = useState<Role>(member.role);
  const save = useMutation({
    mutationFn: () => {
      if (row === undefined) throw new Error('the row of the role was not found');
      return changeRole(row.id, role, row.revision);
    },
    onSuccess: () => {
      toast({ tone: 'success', title: t('places.members.roleChanged', { name: member.user.display_name, role: t(`roles.${role}`) }) });
      onClose();
    },
    onError: (error) => {
      // Somebody changed it first: the row is read again, with its new revision, before the next try.
      if (error instanceof ApiError && error.is('conflict.stale_revision')) void roles.refetch();
    },
  });
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !save.isPending && onClose()}
      title={t('places.members.changeRole', { name: member.user.display_name })}
      description={t('places.members.changeRoleLead', { org: member.domain.name ?? '' })}
      dismissible={!save.isPending}
      footer={
        <>
          <Button disabled={save.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" pending={save.isPending} blocked={row === undefined || role === member.role} onClick={() => save.mutate()}>
            {t('places.members.saveRole')}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4 py-2">
        {roles.isError ? (
          <ErrorState compact error={roles.error} retrying={roles.isRefetching} onRetry={() => void roles.refetch()} />
        ) : roles.isPending ? (
          <SkeletonRows rows={1} />
        ) : (
          <Select label={t('places.invite.role')} value={role} onValueChange={setRole} options={INVITED_ROLES.map((value) => ({ value, ...choiceOption(t, `accessRole.${value}`) }))} />
        )}
        {save.error !== null && <ErrorAlert error={save.error} />}
      </div>
    </Dialog>
  );
}

/** Holds a member out of the org and everything in it until it is lifted; the reason is asked for and kept. */
function Suspend({ org, member, onClose }: { org: Org; member: Member; onClose: () => void }) {
  const { t } = useTranslation();
  const [reason, setReason] = useState('');
  const name = member.user.display_name;
  const hold = useMutation({
    mutationFn: () => suspendMember(org.id, member.user.id, reason),
    onSuccess: () => {
      toast({ tone: 'success', title: t('settings.members.suspend.done', { name, org: org.name }) });
      onClose();
    },
  });
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !hold.isPending && onClose()}
      title={t('settings.members.suspend.title', { name, org: org.name })}
      description={t('settings.members.suspend.lead', { name, org: org.name })}
      dismissible={!hold.isPending}
      footer={
        <>
          <Button disabled={hold.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="danger" pending={hold.isPending} onClick={() => hold.mutate()}>
            {t('settings.members.suspend.submit', { name })}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4 py-2">
        <Textarea label={t('settings.members.suspend.reason')} optional hint={t('settings.members.suspend.reasonHint')} rows={3} value={reason} onChange={(event) => setReason(event.target.value)} />
        {hold.error !== null && <ErrorAlert error={hold.error} />}
      </div>
    </Dialog>
  );
}

/** The invitations still good, each with its form, whom it is for, its role, how often it was used, when it ends, and Revoke. */
function Invitations({ org }: { org: Org }) {
  const { t } = useTranslation();
  const list = useQuery(invitationsQuery(org.id));
  const [revoking, setRevoking] = useState<Invitation | null>(null);
  const now = useNow();
  const revoke = useMutation({
    mutationFn: (invitation: Invitation) => revokeInvitation(invitation.id),
    onSuccess: () => {
      setRevoking(null);
    },
  });
  const pending = (list.data ?? []).filter((invitation) => isPending(invitation, now));
  if (list.isPending) return <SkeletonRows rows={2} />;
  if (list.isError) return <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />;
  if (pending.length === 0) return <EmptyState compact title={t('places.members.noInvitations')} />;
  const columns: Column<Invitation>[] = [
    {
      id: 'for',
      header: t('places.members.invitedFor'),
      cell: (invitation) => (
        <span className="flex flex-col">
          <span className="font-medium wrap-anywhere text-text">{invitation.form === 'email' ? invitation.email : t('places.members.sharedLink')}</span>
          <span className="text-sm text-text-muted">{t(`places.invite.forms.${invitation.form}`)}</span>
        </span>
      ),
    },
    { id: 'role', header: t('places.members.role'), from: 'sm', cell: (invitation) => (invitation.role === null ? null : t(`roles.${invitation.role}`)) },
    { id: 'used', header: t('places.members.used'), from: 'md', cell: (invitation) => t('places.members.usedOf', { uses: invitation.uses, max: invitation.max_uses }) },
    { id: 'ends', header: t('places.members.ends'), from: 'md', cell: (invitation) => <RelativeTime at={invitation.expires_at} /> },
    {
      id: 'revoke',
      header: t('places.members.revoke'),
      hideHeader: true,
      align: 'end',
      cell: (invitation) => (
        <Button size="sm" variant="ghost" onClick={() => setRevoking(invitation)}>
          {t('places.members.revoke')}
        </Button>
      ),
    },
  ];
  return (
    <>
      <Table label={t('places.members.pending')} columns={columns} rows={pending} rowKey={(invitation) => invitation.id} />
      <ConfirmDialog
        open={revoking !== null}
        onOpenChange={(next) => !next && setRevoking(null)}
        title={t('places.members.revokeTitle')}
        confirmLabel={t('places.members.revokeConfirm')}
        danger
        pending={revoke.isPending}
        error={revoke.error ?? undefined}
        onConfirm={() => revoking !== null && revoke.mutate(revoking)}
      >
        {revoking?.form === 'email' ? t('places.members.revokeBodyEmail', { email: revoking.email ?? '' }) : t('places.members.revokeBodyShared')}
      </ConfirmDialog>
    </>
  );
}

interface Acting {
  act: 'role' | 'move' | 'suspend' | 'end';
  member: Member;
}

/** How a member stands with a second step: set up, missing, or signing in by another service (which says nothing of a second step), and whether an org of theirs requires one. */
function SecondStepCell({ member }: { member: Member }) {
  const { t } = useTranslation();
  // A service identity has no human second step.
  if (member.second_step === null) return <span className="text-text-muted">{t('settings.members.secondStep.none')}</span>;
  const required = member.second_step_required === true;
  return (
    <span className="flex flex-wrap items-center gap-1">
      <span className="whitespace-nowrap">{t(`settings.members.secondStep.${member.second_step}`)}</span>
      {required && member.second_step === 'enabled' && <Badge>{t('settings.members.secondStep.required')}</Badge>}
      {required && member.second_step !== 'enabled' && <Badge tone="warning">{t('settings.members.secondStep.needs')}</Badge>}
    </span>
  );
}

/** The column identity also decides where its fact goes when the column cannot fit. */
const MEMBER_ROLE_COLUMN = 'role';

export function Members({ org }: { org: Org }) {
  const { t } = useTranslation();
  const describeError = useErrorText();
  const me = useStore(client.session, (state) => state.userId);
  const [q, setQ] = useState('');
  const [direct, setDirect] = useState(false);
  const [inviting, setInviting] = useState(false);
  const [acting, setActing] = useState<Acting | null>(null);
  const settled = useDebounced(q);
  const mayManage = holds(org.access, 'members.manage');
  const traceScope = scopeOf(org);
  const mayAudit = holds(org.access, 'audit.view');
  const traceEntry = useTraceEntry(traceScope, mayAudit);
  const mayChangeRole = holds(org.access, 'access.manage');
  const root = org.path[0]?.name ?? org.name;
  // Closed means no new members; it does not make the people already here disappear.
  const taking = !org.closed;
  if (inviting && (!mayManage || !taking)) setInviting(false);
  if (acting !== null && !(acting.act === 'role' ? mayChangeRole : acting.act === 'end' ? mayManage || acting.member.user.id === me : mayManage)) setActing(null);
  const list = useInfiniteQuery(membersQuery(org.id, { direct, q: settled }));
  const members = list.data?.pages.flatMap((page) => page.items) ?? [];
  const held = useQuery({ ...suspensionsQuery(org.id), enabled: mayManage });
  const reasons = new Map<string, Suspension>((held.data ?? []).map((suspension) => [suspension.user.id, suspension]));

  const resume = useMutation({
    mutationFn: (member: Member) => resumeMember(org.id, member.user.id),
    onSuccess: (_none, member) => {
      toast({ tone: 'success', title: t('settings.members.suspend.lifted', { name: member.user.display_name, org: org.name }) });
    },
    onError: (error) => toast({ tone: 'danger', title: t('settings.members.suspend.notLifted'), description: describeError(error).message }),
  });

  const columns: Column<Member>[] = [
    {
      id: 'person',
      header: t('places.members.person'),
      cell: (member, shown) => {
        const suspension = reasons.get(member.user.id);
        return (
          <span className="flex min-w-0 flex-col gap-1">
            <span className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
              <TracePerson user={member.user} scope={traceScope} allowed={mayAudit} />
              {member.is_owner && <Badge>{t('places.members.owner')}</Badge>}
              {member.suspended && <Badge tone="warning">{t('places.members.suspended')}</Badge>}
            </span>
            {!shown.has(MEMBER_ROLE_COLUMN) && <span className="text-sm text-text-muted">{t(`roles.${member.role}`)}</span>}
            {member.suspended && suspension !== undefined && (
              <span className="text-sm text-text-muted">
                {suspension.reason === ''
                  ? t('settings.members.suspend.by', { name: suspension.by.display_name })
                  : t('settings.members.suspend.byWhy', { name: suspension.by.display_name, reason: suspension.reason })}
              </span>
            )}
          </span>
        );
      },
    },
    { id: 'domain', header: t('places.members.domain'), from: 'sm', cell: (member) => <span className="wrap-anywhere">{member.domain.name}</span> },
    { id: MEMBER_ROLE_COLUMN, header: <span className="whitespace-nowrap">{t('places.members.role')}</span>, from: 'sm', cell: (member) => <span className="whitespace-nowrap">{t(`roles.${member.role}`)}</span> },
    { id: 'since', header: t('places.members.joined'), from: 'md', cell: (member) => (member.since === null ? null : <RelativeTime at={member.since} className="text-text-muted" />) },
    { id: 'also', header: t('settings.members.also'), from: 'lg', cell: (member) => (member.also === 0 ? <span className="text-text-muted">{t('settings.members.alsoNone')}</span> : t('settings.members.alsoSome', { count: member.also })) },
    // Second step and Last seen are for whoever manages the members alone; nobody else is sent them.
    ...(mayManage
      ? ([
          { id: 'second', header: t('settings.members.secondStep.title'), from: 'lg', cell: (member) => <SecondStepCell member={member} /> },
          {
            id: 'seen',
            header: t('settings.members.lastSeen'),
            from: 'lg',
            cell: (member) =>
              member.last_seen_at === null ? <span className="text-text-muted">{t('settings.members.lastSeenUnknown')}</span> : <RelativeTime at={member.last_seen_at} />,
          },
        ] satisfies Column<Member>[])
      : []),
    {
      id: 'menu',
      header: t('places.members.actions'),
      hideHeader: true,
      className: 'w-14 sm:w-auto',
      align: 'end',
      cell: (member) => {
        const self = member.user.id === me;
        return (
          <Menu
            label={t('places.members.menuOf', { name: member.user.display_name })}
            align="end"
            entries={[
              ...traceEntry(member.user),
              {
                id: 'role',
                label: t('places.members.changeRoleEntry'),
                icon: <ShieldCheck aria-hidden />,
                disabled: mayChangeRole ? false : takes(t, 'access.manage', org.name, org.owner),
                onSelect: () => setActing({ act: 'role', member }),
              },
              {
                id: 'move',
                label: t('settings.members.moveEntry'),
                icon: <MoveRight aria-hidden />,
                disabled: mayManage ? false : takes(t, 'members.manage', org.name, org.owner),
                onSelect: () => setActing({ act: 'move', member }),
              },
              member.suspended
                ? {
                    id: 'resume',
                    label: t('settings.members.resumeEntry'),
                    icon: <Play aria-hidden />,
                    disabled: mayManage ? false : takes(t, 'members.manage', org.name, org.owner),
                    onSelect: () => resume.mutate(member),
                  }
                : {
                    id: 'suspend',
                    label: t('settings.members.suspendEntry'),
                    icon: <Pause aria-hidden />,
                    disabled: !mayManage
                      ? takes(t, 'members.manage', org.name, org.owner)
                      : self
                        ? t('settings.members.cannotHoldSelf')
                        : member.is_owner
                          ? t('settings.members.cannotHoldOwner')
                          : false,
                    onSelect: () => setActing({ act: 'suspend', member }),
                  },
              { type: 'separator', id: 'separator-end' },
              {
                id: 'end',
                label: self ? t('settings.members.leaveEntry', { root }) : t('settings.members.removeEntry', { root }),
                icon: <UserMinus aria-hidden />,
                danger: true,
                disabled: mayManage || self ? false : takes(t, 'members.manage', org.name, org.owner),
                onSelect: () => setActing({ act: 'end', member }),
              },
            ]}
            trigger={<IconButton size="sm" label={t('places.members.menuOf', { name: member.user.display_name })} icon={<Ellipsis aria-hidden />} />}
          />
        );
      },
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <Section
        id="members"
        title={t('settings.org.sectionNames.members')}
        action={
          <PermissionGate allowed={mayManage && taking} mode="block" reason={taking ? takes(t, 'members.manage', org.name, org.owner) : t('settings.members.closed', { name: org.name })}>
            <Button variant="primary" icon={<MailPlus aria-hidden />} onClick={() => setInviting(true)}>
              {t('places.members.invite')}
            </Button>
          </PermissionGate>
        }
      >
        <FilterBar search={<Input
            label={t('places.members.search')}
            hideLabel
            type="search"
            placeholder={t('places.members.searchPlaceholder')}
            value={q}
            onChange={(event) => setQ(event.target.value)}
            fieldClassName="min-w-0 flex-1 basis-[min(100%,14rem)] sm:max-w-(--container-form)"
            autoComplete="off"
          />}>
          {org.org_count > 0 && <Switch label={t('places.members.direct')} checked={direct} onCheckedChange={setDirect} className="items-center gap-3" />}
        </FilterBar>
        {list.isPending ? (
          <SkeletonRows rows={4} />
        ) : list.isError && members.length === 0 ? (
          <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
        ) : members.length === 0 ? settled === '' && org.depth > 0 ? null : (
          <EmptyState compact icon={settled === '' ? <Users aria-hidden /> : <Search aria-hidden />} title={settled === '' ? t('places.members.none') : t('places.members.noMatch', { q: settled })} />
        ) : (
          <div className="flex flex-col gap-2">
            <Table label={t('places.members.title')} columns={columns} rows={members} rowKey={(member) => member.user.id} />
            <Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} />
          </div>
        )}
      </Section>

      {org.depth > 0 && <InheritedMembers org={org} members={members} query={settled} />}

      {mayManage && (
        <Section id="invitations" title={t('places.members.pending')} lead={t('settings.members.invitationsLead')}>
          <InvitationRules org={org} />
          <Invitations org={org} />
        </Section>
      )}

      {mayManage && <Outsiders org={org} />}

      {inviting && mayManage && taking && <InviteDialog target={{ kind: 'org', org }} open onClose={() => setInviting(false)} />}
      {acting?.act === 'role' && <ChangeRole member={acting.member} onClose={() => setActing(null)} />}
      {acting?.act === 'move' && <MoveMember org={org} member={acting.member} onClose={() => setActing(null)} />}
      {acting?.act === 'suspend' && <Suspend org={org} member={acting.member} onClose={() => setActing(null)} />}
      {acting?.act === 'end' && <EndMembership org={org} member={acting.member} self={acting.member.user.id === me} onClose={() => setActing(null)} />}
    </div>
  );
}
