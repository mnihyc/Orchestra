import { CompleteCollection } from '@/ui/CompleteCollection';
/** Enable makes a resource available again at once; running the messages that waited meanwhile is its one explicit choice. */
import { useMutation } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
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
import { ErrorAlert } from '@/ui/ErrorState';
import { StateLine } from '@/ui/StateLine';
import { Table } from '@/ui/Table';
import { toast } from '@/ui/Toast';

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

/** Nothing is read before enabling: a stop that has not finished yet is the server's refusal, shown on its row. */
export function ResourceEnableReview({ resources, onClose }: { resources: readonly EnableResource[]; onClose: () => void }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const [runWaiting, setRunWaiting] = useState(false);
  const [accepted, setAccepted] = useState<ReadonlySet<string>>(new Set());
  const [errors, setErrors] = useState<ReadonlyMap<string, unknown>>(new Map());
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
  const rows = resources.map((resource) => ({ resource, key: keyOf(resource) }));
  const outcome = ({ resource, key }: typeof rows[number]) => accepted.has(key) ? <StateLine compact message={t('source.job.succeeded')} /> : <>
    {!single && <AdminActNotice spec={resource.admin} />}
    {errors.has(key) && <ErrorAlert error={errors.get(key)} />}
  </>;
  return <ConfirmDialog open admin={single ? resources[0]?.admin : undefined} onOpenChange={(open) => !open && onClose()}
    title={single ? t('resourceImpact.enableTitle', { name: resources[0]?.name }) : t('resourceImpact.enableMany', { count: resources.length, number: formatNumber(resources.length, locale) })} confirmLabel={t('resources.act.enable')}
    pending={enable.isPending} error={enable.error} disabled={rows.length === 0} onConfirm={() => enable.mutate()}>
    <div className="flex min-w-0 flex-col gap-3">
      <Checkbox label={t('resourceImpact.runWaiting')} checked={runWaiting} onCheckedChange={setRunWaiting} disabled={enable.isPending} />
      {single ? rows.map((row) => <div key={row.key} className="flex min-w-0 flex-col gap-2">{outcome(row)}</div>) : <CompleteCollection label={t('resources.act.enable')} items={rows} textOf={({resource})=>resource.name}>{(page)=><Table label={t('resources.act.enable')} boundedIdentity rows={page} rowKey={(row) => row.key} columns={[
        {id:'name',header:t('resources.listName'),grow:true,cell:({resource})=>{const subject=resourceSubjectOf(resource.target.object,resource.name);return subject===null?<span className="truncate" title={resource.name}>{resource.name}</span>:<ResourceName subject={subject} hint={false} />;}},
        {id:'outcome',header:t('resourceImpact.result'),cell:(row)=><div className="flex min-w-0 flex-col gap-2">{outcome(row)}</div>},
      ]} />}</CompleteCollection>}
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
