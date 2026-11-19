/** A compact inventory and an exact typed-name confirmation before permanent loss is recorded. */
import { Hint } from '@/features/help/Hint';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { useTranslation } from 'react-i18next';
import { declareLost, lossPlanQuery } from '@/api/loss';
import { NamedChip } from '@/features/people/NamedChip';
import { NotifyPeople } from '@/features/notifications/NotifyPeople';
import { Frame } from '@/features/change/Frame';
import type { LossPlan, LossTarget } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { useActKey } from '@/lib/forms';
import { permissionLabel } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { InlineAlert } from '@/ui/InlineAlert';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';

export function DeclareLostDialog({ target, name, onClose }: { target: LossTarget; name: string; onClose: () => void }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const client = useQueryClient();
  const key = useActKey();
  const plan = useQuery(lossPlanQuery(target));
  const [typed, setTyped] = useState('');
  const [notify, setNotify] = useState<boolean | undefined>();
  const [result, setResult] = useState<LossPlan | null>(null);
  const declare = useMutation({ mutationFn: () => {
    if (plan.data === undefined) throw new Error('the loss plan is unavailable');
    return declareLost({ preflight_id: plan.data.preflight_id, confirm: typed, notify: notify ?? plan.data.notification.default_on }, key({ plan: plan.data.preflight_id }));
  }, onSuccess: (accepted) => { setResult(accepted); void client.invalidateQueries(); }, onError: () => { void plan.refetch(); } });
  const data = result ?? plan.data;
  const threads = (data?.environments.reduce((sum, environment) => sum + environment.threads.length + environment.threads_hidden, 0) ?? 0) + (data?.provider_threads.length ?? 0) + (data?.provider_threads_hidden ?? 0);
  return <Frame open onClose={onClose} title={t('loss.declareTitle', { name })} at={result!==null?'done':declare.isPending?'confirm':'plan'} dismissible={!declare.isPending} footer={<>
    <div className="me-auto min-w-0">{result===null&&<NotifyPeople plan={data?.notification} value={notify} onChange={setNotify} disabled={declare.isPending || plan.isFetching} />}</div>
    <Button disabled={declare.isPending} onClick={onClose}>{t(result===null?'common.cancel':'common.close')}</Button>
    {result===null&&<Button variant="danger" pending={declare.isPending} disabled={declare.isPending || data === undefined || plan.isFetching || typed !== data.name} onClick={() => declare.mutate()}>{t('loss.declare')}</Button>}
  </>}>
    {result!==null&&<StateLine title={t('hostCustody.declared',{name:result.name})}/>}
    {plan.isPending ? <SkeletonRows rows={3} /> : plan.isError ? <ErrorState compact error={plan.error} retrying={plan.isRefetching} onRetry={() => void plan.refetch()} /> : data !== undefined && <div className="flex flex-col gap-2">
      <details><summary>{t('loss.environments', { count: data.environments.length, number: formatNumber(data.environments.length, locale) })} · {t('loss.threads', { count: threads, number: formatNumber(threads, locale) })}</summary><ul className="mt-1 flex flex-col gap-1 text-sm text-text-muted">{data.environments.map((environment) => <li key={environment.environment.id}><details><summary>{environment.environment.name === null ? t('loss.hiddenEnvironment') : <ResourceChip subject={{ kind: 'environment', environment: { id: environment.environment.id, name: environment.environment.name } }} asChild hint={false}><a href={`/resources/environments/${environment.environment.id}`}>{environment.environment.name}</a></ResourceChip>} · {t('loss.threads', { count: environment.threads.length + environment.threads_hidden, number: formatNumber(environment.threads.length + environment.threads_hidden, locale) })}</summary><ul className="ps-3">{environment.threads.map((thread) => <li key={thread.id}><NamedChip kind="thread" path={`/thread/${thread.id}`} id={thread.id} original={thread.name} /></li>)}{environment.threads_hidden > 0 && <li>{t('change.batch.hidden', { count: environment.threads_hidden })}</li>}</ul></details></li>)}</ul></details>
      <details><summary>{t('loss.keptData', { size: data.kept_bytes === null ? '–' : formatBytes(data.kept_bytes, locale) })}</summary><ul className="mt-1 text-sm text-text-muted">{data.environments.filter((environment) => environment.kept_bytes !== null).map((environment) => <li key={environment.environment.id}>{environment.environment.name === null ? t('loss.hiddenEnvironment') : <ResourceChip subject={{ kind: 'environment', environment: { id: environment.environment.id, name: environment.environment.name } }} asChild hint={false}><a href={`/resources/environments/${environment.environment.id}`}>{environment.environment.name}</a></ResourceChip>} · {formatBytes(environment.kept_bytes ?? 0, locale)}</li>)}</ul></details>
      <details><summary>{t('loss.mounts', { count: data.mounts.length, number: formatNumber(data.mounts.length, locale) })} · {t('loss.providers', { count: data.providers.length, number: formatNumber(data.providers.length, locale) })}</summary><ul className="mt-1 text-sm text-text-muted">{data.mounts.map((resource) => <li key={resource.id}>{resource.name === null ? t('loss.hiddenResource') : <ResourceChip subject={{ kind: 'mount', mount: { id: resource.id, name: resource.name } }} asChild hint={false}><a href={`/resources/mounts/${resource.id}`}>{resource.name}</a></ResourceChip>}</li>)}{data.providers.map((resource) => <li key={resource.id}>{resource.name === null ? t('loss.hiddenResource') : <ResourceChip subject={{ kind: 'provider', provider: { id: resource.id, name: resource.name } }} asChild hint={false}><a href={`/resources/providers/${resource.id}`}>{resource.name}</a></ResourceChip>}</li>)}</ul></details>
      {((data.provider_threads.length)+(data.provider_threads_hidden))>0&&<details><summary>{t('loss.providerThreads',{count:(data.provider_threads.length)+(data.provider_threads_hidden),number:formatNumber((data.provider_threads.length)+(data.provider_threads_hidden),locale)})}</summary><ul className="ps-3 text-sm">{(data.provider_threads).map((thread)=><li key={thread.id}><NamedChip kind="thread" path={`/thread/${thread.id}`} id={thread.id} original={thread.name}/></li>)}{(data.provider_threads_hidden)>0&&<li>{t('change.batch.hidden',{count:data.provider_threads_hidden})}</li>}</ul></details>}
      {data.jobs > 0 && <span className="text-sm text-text-muted">{t('loss.jobs', { count: data.jobs, number: formatNumber(data.jobs, locale) })}</span>}
      <StateLine compact message={t('hostCustody.lossEffect')}/>
      {result===null&&<>
        <Input help={<Hint popover id="loss-declare" />} label={t('loss.typeName', { name: data.name })} value={typed} onChange={(event) => setTyped(event.target.value)} autoComplete="off" disabled={declare.isPending}/>
        {(data.admin_grants??[]).length>0&&<InlineAlert tone="warning"><span>{t('loss.adminGrant')}</span><ul>{(data.admin_grants??[]).map((grant)=><li key={`${grant.object.kind}:${grant.object.id}:${grant.permission}`}>{grant.name} · {permissionLabel(grant.permission)}</li>)}</ul></InlineAlert>}
      </>}
    </div>}
    {declare.error!==null&&<ErrorAlert error={declare.error}/>}
  </Frame>;
}
