/** The dialog that moves a member to another org of the tree: what they stop and start reaching, what of theirs stays, and the role they will hold there.
 *
 * A member belongs to one org of the tree, their domain, and holds a role there. Moving them changes
 * what they inherit and nothing that names them: the roles they were given by name, the groups they
 * are in and what they own stay where they are. As soon as a destination is chosen the server is asked
 * what the move would do, and the answer is written out before anything is moved. Where the caller
 * lacks what the move needs at either end, the dialog says what is missing and whom to ask.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { moveMember, previewMemberMove } from '@/api/directory';
import { INVITED_ROLES } from '@/features/org/Invite';
import type { Member, MemberMovePlan, Org, Role, TreeNode } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { useOrgTree } from '@/realtime/stores/tree';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState, MissingAccessList } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Select } from '@/ui/Select';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';

/** One org of the tree as a place to move to: where it lies by name. */
export interface Place {
  id: string;
  path: string;
  parentId: string | null;
}

/** The orgs of a tree in the order a person reads them, each with the names of the orgs it lies in. */
export function orgPlaces(rows: ReadonlyMap<string, TreeNode>, separator: string): Place[] {
  const orgs = [...rows.values()].filter((row) => row.kind === 'org' && !row.deleted);
  const byParent = new Map<string | null, TreeNode[]>();
  for (const row of orgs) byParent.set(row.parent_id, [...(byParent.get(row.parent_id) ?? []), row]);
  const found: Place[] = [];
  const walk = (row: TreeNode, above: string[]) => {
    const path = [...above, row.name];
    found.push({ id: row.id, path: path.join(separator), parentId: row.parent_id });
    for (const child of (byParent.get(row.id) ?? []).sort((one, other) => one.name.localeCompare(other.name))) walk(child, path);
  };
  for (const root of byParent.get(null) ?? []) walk(root, []);
  return found;
}

/** Whether `ancestor` is the org `place` or one it lies in, by the parents the places have. */
export function liesIn(places: readonly Place[], placeId: string, ancestor: string): boolean {
  const byId = new Map(places.map((place) => [place.id, place]));
  for (let at = byId.get(placeId), steps = 0; at !== undefined && steps < 64; steps += 1) {
    if (at.id === ancestor) return true;
    at = at.parentId === null ? undefined : byId.get(at.parentId);
  }
  return false;
}

/** What a set of counts comes to in words: "3 places, 2 resources and 12 documents"; nothing for nothing. */
function Reach({ reach }: { reach: MemberMovePlan['loses'] }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const parts = [
    reach.nodes > 0 ? t('settings.members.move.reach.nodes', { count: reach.nodes }) : null,
    reach.resources > 0 ? t('settings.members.move.reach.resources', { count: reach.resources }) : null,
    reach.documents > 0 ? t('settings.members.move.reach.documents', { count: reach.documents }) : null,
  ].filter((part): part is string => part !== null);
  return <>{parts.length === 0 ? t('settings.members.move.reach.nothing') : new Intl.ListFormat(locale, { style: 'long', type: 'conjunction' }).format(parts)}</>;
}

/** What the move would do, written out. */
function Plan({ plan, member, warning }: { plan: MemberMovePlan; member: Member; warning: boolean }) {
  const { t } = useTranslation();
  const name = member.user.display_name;
  const owned = plan.stays.nodes;
  const ownedCount = owned.orgs + owned.projects + owned.topics + owned.threads + owned.documents;
  return (
    <div className="flex flex-col gap-3 text-base text-text">
      {plan.missing.length > 0 && (
        <InlineAlert tone="danger" title={t('settings.members.move.missing')}>
          <MissingAccessList missing={plan.missing} className="mt-1" />
        </InlineAlert>
      )}
      <p>{t('settings.members.move.becomes', { name, from: plan.from.name ?? '', to: plan.to.name ?? '' })}</p>
      <p>
        {t('settings.members.move.reaches', { name })} <Reach reach={plan.loses} />
        {t('settings.members.move.starts')} <Reach reach={plan.gains} />.
      </p>
      <p>
        {t('settings.members.move.stays', { name })}
        {plan.stays.roles.length > 0 && ` ${t('settings.members.move.staysRoles', { count: plan.stays.roles.length })}`}
        {ownedCount > 0 && ` ${t('settings.members.move.staysOwns', { count: ownedCount, where: plan.from.name ?? '' })}`}
      </p>
      {plan.waiting.length > 0 && <p className="text-text-muted">{t('settings.members.move.waiting', { count: plan.waiting.length })}</p>}
      {warning && <InlineAlert tone="warning">{t('settings.members.move.adminOfAll', { name, to: plan.to.name ?? '' })}</InlineAlert>}
    </div>
  );
}

export interface MoveMemberProps {
  /** The org the list was opened on. */
  org: Org;
  member: Member;
  onClose: () => void;
}

export function MoveMember({ org, member, onClose }: MoveMemberProps) {
  const { t } = useTranslation();
  const tree = useOrgTree(org.org_id);
  const places = useMemo(() => orgPlaces(tree.nodes, t('settings.place.separator')), [tree.nodes, t]);
  const [to, setTo] = useState('');
  const [role, setRole] = useState<Role | null>(null);
  const name = member.user.display_name;

  const preview = useQuery({
    queryKey: ['member-move-plan', org.id, member.user.id, to, role] as const,
    queryFn: ({ signal }) => previewMemberMove(org.id, member.user.id, to, role, signal),
    enabled: to !== '',
    staleTime: 0,
    retry: false,
  });
  const plan = preview.data;
  const chosenRole: Role = role ?? plan?.role ?? member.role;
  // Moving an admin of a department up to the org above makes them an admin of all of it.
  const warning = chosenRole === 'admin' && to !== '' && liesIn(places, member.domain.id, to);

  const move = useMutation({
    mutationFn: () => moveMember(org.id, member.user.id, to, role),
    onSuccess: (moved) => {
      toast({ tone: 'success', title: t('settings.members.move.done', { name, to: moved.domain.name ?? '' }) });
      onClose();
    },
  });

  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !move.isPending && onClose()}
      title={t('settings.members.move.title', { name })}
      description={t('settings.members.move.lead', { name, from: member.domain.name ?? '' })}
      size="lg"
      dismissible={!move.isPending}
      footer={
        <>
          <Button disabled={move.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" pending={move.isPending} blocked={to === '' || plan === undefined || plan.missing.length > 0} onClick={() => move.mutate()}>
            {t('settings.members.move.submit', { name })}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4 py-2">
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-[1fr_12rem]">
          <Select
            label={t('settings.members.move.to')}
            value={to}
            placeholder={t('settings.members.move.choose')}
            onValueChange={(next) => {
              setTo(next);
              setRole(null);
            }}
            options={places.map((place) => ({
              value: place.id,
              label: place.path,
              disabled: place.id === member.domain.id,
              ...(place.id === member.domain.id ? { description: t('settings.members.move.here') } : {}),
            }))}
          />
          <Select
            label={t('settings.members.move.role')}
            value={chosenRole}
            onValueChange={setRole}
            options={INVITED_ROLES.map((value) => ({ value, ...choiceOption(t, `accessRole.${value}`) }))}
          />
        </div>
        {to !== '' && preview.isPending && (
          <SkeletonGroup className="flex flex-col gap-2">
            <Skeleton className="h-4 w-3/4" />
            <Skeleton className="h-4 w-2/3" />
          </SkeletonGroup>
        )}
        {to !== '' && preview.isError && <ErrorState compact error={preview.error} retrying={preview.isRefetching} onRetry={() => void preview.refetch()} />}
        {plan !== undefined && <Plan plan={plan} member={member} warning={warning} />}
        {move.error !== null && <ErrorAlert error={move.error} />}
      </div>
    </Dialog>
  );
}
