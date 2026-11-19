/** Enable reviews the existing waiting queue before making a resource available again. */
import { useMutation, useQueries } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { resourceImpactQuery } from '@/api/resource-impact';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { ResourceName } from '@/features/resource-facts/ResourceFacts';
import { resourceSubjectOf } from '@/features/resource-facts/facts';
import type { FactTarget } from '@/generated/api-types';
import type { AdminActSpec } from '@/ui/AdminAct';
import { AdminActNotice } from '@/ui/AdminAct';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { Checkbox } from '@/ui/Checkbox';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { Table } from '@/ui/Table';
import { toast } from '@/ui/Toast';
import { ActivityCount } from './Impact';

export interface EnableResource {
  target: FactTarget;
  name: string;
  admin?: AdminActSpec;
  enable: (runWaiting: boolean) => Promise<unknown>;
}

export function ResourceEnableDialog({ resource, onClose }: { resource: EnableResource; onClose: () => void }) {
  return <ResourceEnableReview resources={[resource]} onClose={onClose} />;
}

const keyOf = (resource: EnableResource) => `${resource.target.object.id}:${resource.target.model_id ?? ''}`;

export function ResourceEnableReview({ resources, onClose }: { resources: readonly EnableResource[]; onClose: () => void }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const [runWaiting, setRunWaiting] = useState(false);
  const [accepted, setAccepted] = useState<ReadonlySet<string>>(new Set());
  const [errors, setErrors] = useState<ReadonlyMap<string, unknown>>(new Map());
  const impacts = useQueries({ queries: resources.map((resource) => resourceImpactQuery(resource.target)) });
  const single = resources.length === 1;
  const enable = useMutation({ mutationFn: async () => {
    let failed = false;
    for (const resource of resources) {
      const key = keyOf(resource);
      if (accepted.has(key)) continue;
      try {
        await resource.enable(runWaiting);
        setAccepted((before) => new Set(before).add(key));
        setErrors((before) => { const next = new Map(before); next.delete(key); return next; });
      } catch (error: unknown) {
        failed = true;
        setErrors((before) => new Map(before).set(key, error));
      }
    }
    return !failed;
  }, onSuccess: (complete) => {
    if (!complete) return;
    toast({ tone: 'success', title: single ? t('resourceImpact.enabled', { name: resources[0]?.name }) : t('resourceImpact.enabledMany', { count: resources.length, number: formatNumber(resources.length, locale) }) });
    onClose();
  } });
  const rows = resources.map((resource, index) => ({ resource, impact: impacts[index], key: keyOf(resource) }));
  const effects = ({ resource, impact, key }: typeof rows[number]) => accepted.has(key) ? <StateLine compact message={t('source.job.succeeded')} /> : <>
    {impact === undefined || impact.isPending ? <SkeletonRows rows={2} /> : impact.isError ? <ErrorState compact error={impact.error} onRetry={() => void impact.refetch()} /> : <>
      <ActivityCount group={impact.data.activity.waiting} kind="waiting" />
      <ActivityCount group={impact.data.activity.stopped} kind="stopped" />
      {impact.data.stopping && <StateLine compact message={t('resourceImpact.stopping')} />}
    </>}
    {!single && <AdminActNotice spec={resource.admin} />}
    {errors.has(key) && <ErrorAlert error={errors.get(key)} />}
  </>;
  return <ConfirmDialog open admin={single ? resources[0]?.admin : undefined} onOpenChange={(open) => !open && onClose()}
    title={single ? t('resourceImpact.enableTitle', { name: resources[0]?.name }) : t('resourceImpact.enableMany', { count: resources.length, number: formatNumber(resources.length, locale) })} confirmLabel={t('resources.act.enable')}
    pending={enable.isPending} error={enable.error} disabled={rows.length === 0 || rows.some(({key, impact}) => !accepted.has(key) && (impact === undefined || !impact.isSuccess || impact.data.stopping))} onConfirm={() => enable.mutate()}>
    <div className="flex min-w-0 flex-col gap-3">
      {rows.every(({impact})=>impact?.isSuccess===true) && <Checkbox label={t('resourceImpact.runWaiting')} checked={runWaiting} onCheckedChange={setRunWaiting} disabled={enable.isPending} />}
      {single ? rows.map((row) => <div key={row.key} className="flex min-w-0 flex-col gap-2">{effects(row)}</div>) : <Table label={t('resources.act.enable')} boundedIdentity rows={rows} rowKey={(row) => row.key} columns={[
        {id:'name',header:t('resources.listName'),grow:true,cell:({resource})=>{const subject=resourceSubjectOf(resource.target.object,resource.name);return subject===null?<span className="truncate" title={resource.name}>{resource.name}</span>:<ResourceName subject={subject} hint={false} />;}},
        {id:'waiting',header:t('resourceImpact.names'),cell:(row)=><div className="flex min-w-0 flex-col gap-2">{effects(row)}</div>},
      ]} />}
      <ArticleGuideLink article="pause-and-switch" heading="enable" title={t('resourceImpact.guide')} />
    </div>
  </ConfirmDialog>;
}

export function useResourceEnable() {
  const [resources, setResources] = useState<readonly EnableResource[] | null>(null);
  return {
    ask: (next: EnableResource) => setResources([next]),
    askMany: (next: readonly EnableResource[]) => setResources(next),
    dialogs: resources === null ? null : <ResourceEnableReview key={resources.map(keyOf).join(',')} resources={resources} onClose={() => setResources(null)} />,
  };
}
