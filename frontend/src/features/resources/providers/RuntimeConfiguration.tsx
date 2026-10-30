/** Shared provider unit configuration, write-only secret values and a reviewed replacement plan. */
import { Hint } from '@/features/help/Hint';
import { useMutation, useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { NotifyPeople } from '@/features/notifications/NotifyPeople';
import { StateLine } from '@/ui/StateLine';
import { setProviderSecret } from '@/api/resources';
import { targetSettingsQuery } from '@/api/settings';
import { planRuntime, updateRuntime } from '@/api/provider-runtime';
import type { Provider, ProviderUpdatePlan } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { newActKey } from '@/lib/requests';
import { Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { AdminActNotice, adminSpec } from '@/ui/AdminAct';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { PasswordInput } from '@/ui/Input';
import { InlineAlert } from '@/ui/InlineAlert';
import { jobQuery } from '@/api/forks';
import { ProviderGuideLink } from '@/features/help/ProviderGuideLink';
import { JobProgress } from '@/features/change/Progress';
import { UnitConfiguration } from '@/features/resources/UnitConfiguration';
import { record } from '@/features/resources/UnitCommands';
import { ProviderHome } from '@/features/resources/providers/Home';
import { Checkbox } from '@/ui/Checkbox';
import { RuntimeChange } from '@/features/resources/TechnicalValue';
import { TargetBuilds } from '@/features/change/TargetBuilds';
import { UpdateNotify } from '@/features/change/UpdateNotify';

function RuntimeJob({ id }: { id:string }) {
  const { t } = useTranslation();
  const job = useQuery(jobQuery(id));
  return job.error ? <ErrorAlert error={job.error} /> : job.data === undefined ? null : <JobProgress job={job.data} heading={t('providerRuntime.progress')} reconnecting={job.failureCount > 0} />;
}

function RuntimeSecret({ provider, name }: { provider: Provider; name: string }) {
  const { t } = useTranslation();
  const [value, setValue] = useState('');
  const save = useMutation({ mutationFn: () => setProviderSecret(provider.id, `runtime:${name}`, value), onSuccess: () => {
    setValue('');
  } });
  return <form className="flex flex-col gap-2" onSubmit={(event) => { event.preventDefault(); save.mutate(); }}>
    <div className="flex flex-wrap items-end gap-2"><PasswordInput label={name} value={value} onChange={(event) => setValue(event.target.value)} autoComplete="new-password" disabled={save.isPending} /><Button admin={adminSpec(provider, 'provider.manage')} type="submit" pending={save.isPending} disabled={value === ''}>{t('resources.edit.save')}</Button></div>
    <p className="text-xs text-text-muted">{t(provider.secrets_set.includes(`runtime:${name}`) ? 'providerRuntime.secretSet' : 'providerRuntime.secretMissing')}</p>
    {save.error && <ErrorAlert error={save.error} />}
  </form>;
}

export function RuntimeConfiguration({ provider }: { provider: Provider }) {
  const { t } = useTranslation();
  const settings = useQuery(targetSettingsQuery({ kind: 'provider', id: provider.id }));
  const manages = holds(provider.access, 'provider.manage');
  const owns = holds(provider.access, 'access.own');
  const own = record(provider.runtime_config);
  const [notify,setNotify]=useState<boolean|undefined>();
  const recovering=provider.runtime_unavailable !== undefined && provider.runtime_unavailable !== null;
  const [review, setReview] = useState<ProviderUpdatePlan | null>(null);
  const [job, setJob] = useState<string | null>(null);
  const [actKey, setActKey] = useState(newActKey);
  const [copyDirectories, setCopyDirectories] = useState(false);
  const preview = useMutation({ mutationFn: () => planRuntime(provider.id,copyDirectories), onSuccess:(plan) => { setActKey(newActKey()); setReview(plan); } });
  const apply = useMutation({ mutationFn: (preflight: string) => updateRuntime(provider.id, preflight, actKey, review?.recovery===true?(notify??review.notification?.default_on??false):(notify??true)), onSuccess: (row) => { setJob(row.id); setReview(null); } });
  const names = settings.data?.find((setting) => setting.key === 'runtime.secret_names')?.value;
  const support=provider.runtime_support;
  const home=settings.data?.find((setting)=>setting.key==='runtime.home');
  return <Section id="provider-runtime-config" title={t('providerRuntime.configure')} action={recovering ? <Hint popover id="loss-providers" /> : undefined} lead={t(support?.lifecycle === 'unit' ? 'providerRuntime.reviewHelp' : 'providerRuntime.directHelp')}>
    <ProviderGuideLink guide="commands-after-start" />
    {home !== undefined && <ProviderHome key={`${provider.id}:${home.own?.revision ?? 0}`} provider={provider} setting={home} />}
    <UnitConfiguration pool="provider" id={provider.id} placement={provider.placement} owns={owns}
      lifecycle={recovering?'unit':support?.lifecycle ?? null} resourceSupport={support?.resource_limits} commands={support?.commands_after_start ?? null} afterUp={support?.after_up ?? null}
      templateId={provider.runtime?.template.id ?? (typeof own['runtime.template_id'] === 'string' ? own['runtime.template_id'] : undefined)}
      jobId={provider.runtime?.job?.id} failureDetail={provider.status_detail} runtime={provider.runtime_changes}
      place={{ name: provider.name, owner: provider.owner, access: provider.access, kind: null }} />
    {(provider.runtime !== null || recovering) && manages && <Button pending={preview.isPending} onClick={() => { apply.reset(); preview.mutate(); }}>{t(recovering?'loss.recreate':'providerRuntime.review')}</Button>}
    {provider.runtime !== null && !recovering && manages && <Checkbox label={t('environmentData.moveCopy')} checked={copyDirectories} disabled={preview.isPending || apply.isPending} onCheckedChange={(checked) => { setCopyDirectories(checked); setReview(null); }} />}
    {preview.error && <ErrorAlert error={preview.error} />}
    {manages && Array.isArray(names) && names.filter((name): name is string => typeof name === 'string').map((name) => <RuntimeSecret key={name} provider={provider} name={name} />)}
    {job !== null && <RuntimeJob id={job} />}
    <Dialog open={review !== null} onOpenChange={(open) => { if (!open && !apply.isPending) setReview(null); }} title={t(review?.recovery===true?'loss.recreate':'providerRuntime.review')} footer={<><div className="me-auto min-w-0"><>{review?.recovery===true ? <NotifyPeople plan={review.notification} value={notify} onChange={setNotify} disabled={apply.isPending}/> : <UpdateNotify audience={review?.notify} checked={notify??true} onChange={setNotify} disabled={apply.isPending}/>}</></div><Button data-autofocus disabled={apply.isPending} onClick={() => setReview(null)}>{t('common.cancel')}</Button><Button variant="primary" pending={apply.isPending} disabled={review === null || review.blockers.length > 0} onClick={() => { if (review !== null) apply.mutate(review.preflight_id); }}>{t(review?.recovery===true?'loss.recreate':'providerRuntime.apply')}</Button></>}>
      <AdminActNotice spec={adminSpec(provider, 'provider.manage')} />
      {review !== null && <div className="flex flex-col gap-3">
        <TargetBuilds updates={review.updates} programs={review.target_programs} />
        {review.recovery&&<StateLine compact message={t('loss.providerFilesLost')}/>}
        {(review.admin_grants??[]).length>0&&<StateLine compact tone="warning" message={t('loss.adminGrant')}/>}
        <ul className="flex flex-col gap-3">{review.updates.map((change,index) => <li key={index}><RuntimeChange change={change} /></li>)}</ul>
        {review.kept.length>0 && <section><h3 className="text-sm font-medium">{t('providerRuntime.kept')}</h3><ul className="text-sm">{review.kept.map((value,index)=><li key={index} className="wrap-anywhere">{value}</li>)}</ul></section>}
        {review.lost.length>0 && <section><h3 className="text-sm font-medium">{t('providerRuntime.notKept')}</h3><ul className="text-sm">{review.lost.map((value,index)=><li key={index} className="wrap-anywhere">{value}</li>)}</ul></section>}
        <ul>{review.threads.map((thread,index) => <li key={index} className="flex flex-wrap gap-x-3 gap-y-1"><span>{thread.thread?.name ?? t('providerRuntime.privateThread')}</span>{thread.turn_running && <span>{t('providerRuntime.runningTurn')}</span>}</li>)}</ul>
        {review.blockers.map((blocker,index) => <InlineAlert key={index} tone="warning">{blocker.message}</InlineAlert>)}{apply.error && <ErrorAlert error={apply.error} />}
      </div>}
    </Dialog>
  </Section>;
}
