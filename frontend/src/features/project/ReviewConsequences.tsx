/** Complete per-place archive, move and deletion consequences, shared by single and selected-row reviews. */
import { useTranslation } from 'react-i18next';
import { Account } from '@/features/access/ChangePreview';
import { PlaceLink } from '@/features/navigation/places';
import { useNodeLabel } from '@/features/navigation/placeLabels';
import { Facts } from '@/features/resources/Facts';
import type { NodeArchivePlan, NodeDeletePlan, NodeMovePlan, NodeRef, NodeOrSystem, PlanBlocker } from '@/generated/api-types';
import { permissionLabel } from '@/lib/permissions';
import { InlineAlert } from '@/ui/InlineAlert';
import { ErrorAlert } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';

export function ReviewPlace({ place }: { place: Exclude<NodeOrSystem, 'system'> }) {
  const { t } = useTranslation();
  if (place.kind === 'document_container') return <span className="wrap-anywhere">{place.redacted || place.name === null ? t('placeReview.private') : place.name}</span>;
  return <ReviewNode place={place} />;
}

function ReviewNode({ place }: { place: NodeRef }) {
  const { t } = useTranslation();
  const name = useNodeLabel(place.id, place.redacted ? undefined : place.name ?? undefined);
  return name === undefined ? <span>{t('placeReview.private')}</span> : <PlaceLink place={place} className="text-accent-text underline wrap-anywhere">{name}</PlaceLink>;
}

export function ReviewBlockers({ blockers }: { blockers: readonly PlanBlocker[] }) {
  return blockers.length === 0 ? null : <ul className="flex flex-col gap-2">{blockers.map((blocker, index) => <li key={`${blocker.code}:${index}`}><ErrorAlert error={blocker} /></li>)}</ul>;
}

export function ArchiveConsequences({ plan }: { plan: NodeArchivePlan }) {
  const { t } = useTranslation();
  const unknown = t('placeReview.notVisible');
  return <div className="flex min-w-0 flex-col gap-3 text-sm">
    <p>{t(plan.archiving ? 'placeReview.archiveAbout' : 'placeReview.unarchiveAbout')}</p>
    <p>{t('placeReview.documentsReadOnly', { count: plan.counts.documents })}</p>
    <ul className="flex flex-col gap-1" aria-label={t('placeReview.subtree')}>{plan.subtree.map((item) => <li key={item.node.id} className="flex flex-wrap items-baseline gap-x-2"><span>{t(`finder.kinds.${item.node.kind}`)}</span><ReviewPlace place={item.node} /><span className="text-text-muted">{t(item.read_only_after ? 'placeReview.readOnly' : 'placeReview.ordinaryRights')}{item.own_archived && !plan.archiving ? ` · ${t('placeReview.ownArchiveKept')}` : ''}</span></li>)}</ul>
    {plan.archiving ? <ul className="flex flex-col gap-3" aria-label={t('placeReview.work')}>{plan.work.map((item) => <li key={item.thread.id} className="flex flex-col gap-1"><ReviewPlace place={item.thread} /><Facts rows={[
      { name: t('placeReview.turns'), value: item.turns ?? unknown }, { name: t('placeReview.inputs'), value: item.inputs ?? unknown },
      { name: t('placeReview.approvals'), value: item.approvals ?? unknown }, { name: t('placeReview.leases'), value: item.leases ?? unknown },
      { name: t('placeReview.session'), value: item.session === null ? unknown : t(item.session ? 'placeReview.willClose' : 'placeReview.noneOpen') },
      { name: t('placeReview.links'), value: item.links ?? unknown },
    ]} /></li>)}</ul> : <InlineAlert tone="info">{t('placeReview.noRestart')}</InlineAlert>}
    <p>{t('placeReview.environmentsKept')}</p>
    <ul className="flex flex-col gap-1">{plan.environments.map((environment) => <li key={environment.id}>{environment.redacted ? t('placeReview.privateEnvironment') : environment.name ?? t('placeReview.privateEnvironment')}</li>)}</ul>
    <p>{t('placeReview.copiesKept', { count: plan.copies_kept })}</p><p>{t('placeReview.liveSharesKept', { count: plan.live_shares_kept })}</p>
    <p>{t('placeReview.linksKept')}</p>
    <p className="text-text-muted">{t('placeReview.expires')}{' '}<RelativeTime at={plan.expires_at} /></p>
    <ReviewBlockers blockers={plan.blockers} />
  </div>;
}

export function MoveConsequences({ plan }: { plan: NodeMovePlan }) {
  const { t } = useTranslation();
  const value = (item: unknown) => item === null ? t('placeReview.unset') : JSON.stringify(item);
  return <div className="flex min-w-0 flex-col gap-3 text-sm">
    <Facts rows={[{ name: t('placeReview.from'), value: <ReviewPlace place={plan.from} /> }, { name: t('placeReview.to'), value: <ReviewPlace place={plan.to} /> }]} />
    <Account preview={plan.access} />
    <p>{t('placeReview.roles', { gaining: plan.roles.gaining, losing: plan.roles.losing })}</p>
    <p>{t('placeReview.addresses', { members: plan.addresses.members, resources: plan.addresses.resources })}</p>
    <ul className="flex flex-col gap-2" aria-label={t('placeReview.settings')}>{plan.settings.map((setting) => <li key={setting.key} className="flex flex-col gap-1"><code className="wrap-anywhere">{setting.key}</code><span className="wrap-anywhere">{value(setting.from)}{' → '}{value(setting.to)}</span>{setting.locked_by !== null && <span>{t('placeReview.lockedBy')}{' '}{setting.locked_by === 'system' ? t('placeReview.system') : <ReviewPlace place={setting.locked_by} />}</span>}</li>)}</ul>
    <p>{t('places.move.plan.untouched')}</p><ReviewBlockers blockers={plan.blockers} />
  </div>;
}

export function DeleteConsequences({ plan }: { plan: NodeDeletePlan }) {
  const { t } = useTranslation();
  return <div className="flex flex-col gap-3 text-sm">
    <Facts rows={[
      ...(['orgs', 'projects', 'topics', 'threads', 'documents'] as const).map((kind) => ({ name: t(`placeReview.counts.${kind}`), value: plan.counts[kind] })),
      { name: t('placeReview.purgeAt'), value: <RelativeTime at={plan.purge_at} /> },
      { name: t('placeReview.restoreBy'), value: <span>{permissionLabel(plan.restore_by.permission)}{' · '}{plan.restore_by.place === 'system' ? t('placeReview.system') : <ReviewPlace place={plan.restore_by.place} />}</span> },
    ]} />
    <p>{t('placeReview.deleteAbout')}</p><p className="text-text-muted">{t('placeReview.expires')}{' '}<RelativeTime at={plan.expires_at} /></p>
  </div>;
}
