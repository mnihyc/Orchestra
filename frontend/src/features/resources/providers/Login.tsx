/** A sign-in rendered from its state: what to show, what to ask; an existing sign-in copied in, as text or by its path in the runtime, with what the vendor says of one sign-in in two places; signing in again or another way; a token's end.
 * Native method fields and steps come from the provider's harness; values are cleared after sending.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { answerProviderLogin, cancelProviderLogin, loginStateQuery, retryProvider, startProviderLogin } from '@/api/provider-plan';
import { providerSetupQuery } from '@/api/resources';
import { CommandBox } from '@/features/resources/Facts';
import type { LoginMethod, Provider } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { FieldForm, initialValues, missingFields, textValues, type FormValues } from '@/ui/FieldForm';
import { InlineAlert } from '@/ui/InlineAlert';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { SkeletonRows } from '@/ui/Skeleton';
import { Section } from '@/shell/Page';
import { RelativeTime } from '@/ui/RelativeTime';
import { HelpHint } from '@/ui/HelpHint';

/** Native steps use the current login state; only the selected method owns a draft. */

/** Prefer an interactive screen flow, including when a stored descriptor predates that default. */
export function preferredLoginMethod(methods: readonly LoginMethod[]): LoginMethod | undefined {
  return methods.find((method)=>method.kind==='interactive' && method.recommended)
    ?? methods.find((method)=>method.kind==='interactive' && method.fields.length===0)
    ?? methods.find((method)=>method.recommended) ?? methods[0];
}

function MethodForm({ provider, method }: { provider:Provider; method:LoginMethod }) {
  const { t } = useTranslation();
  const cache=useQueryClient();
  const [values, setValues] = useState<FormValues>(() => initialValues(method.fields));
  const start = useMutation({ mutationFn: () => startProviderLogin(provider.id, method.id, textValues(method.fields, values)), onSuccess: (state) => { setValues(initialValues(method.fields)); cache.setQueryData(loginStateQuery(provider.id).queryKey,state); } });
  return <form className="flex flex-col gap-3" onSubmit={(event) => { event.preventDefault(); start.mutate(); }}>
    {method.requires !== null && <InlineAlert tone="info">{method.requires}</InlineAlert>}
    <FieldForm fields={method.fields} values={values} onChange={(name,value) => setValues((before) => ({ ...before,[name]:value }))} disabled={start.isPending} />
    <Button type="submit" className="self-start" variant="primary" pending={start.isPending} disabled={missingFields(method.fields,values).length > 0}>{t('common.continue')}</Button>
    {start.error && <ErrorAlert error={start.error} />}
  </form>;
}

function NativeLogin({ provider,allowed }: { provider:Provider;allowed:boolean }) {
  const { t } = useTranslation();
  const cache=useQueryClient();
  const setup = useQuery({...providerSetupQuery(provider.home.id,provider.id),enabled:allowed});
  const progress = useQuery(loginStateQuery(provider.id));
  const methods = setup.data?.subscriptions.find((row) => row.agent.id === provider.agent?.id)?.methods ?? [];
  const [chosen, setChosen] = useState<string | null>(null);
  const [values, setValues] = useState<FormValues>({});
  const method = methods.find((row) => row.id === chosen) ?? preferredLoginMethod(methods);
  const answer = useMutation({ mutationFn: () => answerProviderLogin(provider.id,textValues(progress.data?.asks ?? [],values)), onSuccess:(state) => { setValues({}); cache.setQueryData(loginStateQuery(provider.id).queryKey,state); } });
  const cancel = useMutation({ mutationFn:() => cancelProviderLogin(provider.id),onSuccess:(state)=>{setValues({});cache.setQueryData(loginStateQuery(provider.id).queryKey,state);} });
  if ((allowed && setup.isPending) || progress.isPending) return <SkeletonRows rows={4} />;
  if ((allowed && setup.isError) || progress.isError) return <ErrorState compact error={setup.error ?? progress.error} onRetry={() => { if(allowed) void setup.refetch(); void progress.refetch(); }} />;
  const state = progress.data;
  const active = state.state === 'starting' || state.state === 'awaiting_user';
  return <div className="flex flex-col gap-4">
    <p role="status" className="text-sm text-text-muted">{t(`providerLogin.states.${state.state}`)}</p>
    {state.error !== null && <InlineAlert tone="warning">{state.error}</InlineAlert>}
    {state.shows.map((show,index) => show.kind === 'uri' && /^https?:\/\//i.test(show.value) ? <a key={index} href={show.value} rel="noreferrer" target="_blank" className="text-accent underline">{show.label}</a> : <CommandBox key={index} command={show.value} label={show.label} />)}
    {state.expires_at !== null && <p className="text-sm text-text-muted">{t('providerLogin.expires')} <RelativeTime at={state.expires_at} /></p>}
    {active ? <>
      {allowed && state.asks !== null && state.asks.length > 0 && <form className="flex flex-col gap-3" onSubmit={(event) => { event.preventDefault(); answer.mutate(); }}><FieldForm fields={state.asks} values={values} onChange={(name,value) => setValues((before) => ({ ...before,[name]:value }))} disabled={answer.isPending} /><Button type="submit" pending={answer.isPending} disabled={missingFields(state.asks,values).length > 0}>{t('common.continue')}</Button>{answer.error && <ErrorAlert error={answer.error} />}</form>}
      <Button className="self-start" pending={cancel.isPending} onClick={() => cancel.mutate()}>{t('common.cancel')}</Button>{cancel.error && <ErrorAlert error={cancel.error} />}
    </> : !allowed ? null : method === undefined ? <InlineAlert tone="info">{t('providerLogin.noMethods')}</InlineAlert> : <>
      <ChoiceLocator label={t('providerLogin.method')} hint={method.summary === method.title ? undefined : method.summary} value={method.id} onValueChange={setChosen} options={methods.map((row) => ({ value: row.id, label:row.title, description:row.summary===row.title?undefined:row.summary }))} />
      <MethodForm key={method.id} provider={provider} method={method} />
    </>}
  </div>;
}

export function ProviderLogin({ provider }: { provider:Provider }) {
  const { t } = useTranslation();
  const cache=useQueryClient();
  const retry = useMutation({ mutationFn:() => retryProvider(provider.id),onSettled:()=>{
    void cache.invalidateQueries({queryKey:providerSetupQuery(provider.home.id,provider.id).queryKey});
    void cache.invalidateQueries({queryKey:loginStateQuery(provider.id).queryKey});
  } });
  const support=provider.runtime_support?.sign_in;
  return <Section id="provider-signin" title={t('providers.signin.title')}>
    <InlineAlert tone="info">{support?.home_owned === true ? t('providerLogin.ownedHome') : support?.allowed === true ? t('providerLogin.permittedHome') : support === undefined ? t('providerLogin.homeUnread') : t('providerLogin.homeDenied')}</InlineAlert>
    <NativeLogin provider={provider} allowed={support?.allowed === true} />
    {provider.facts.credential?.yields === 'token' && <p className="flex flex-wrap gap-x-3 gap-y-1 text-sm text-text-muted"><span>{t('providerPlan.nonrenewal')}</span>{provider.facts.credential.expires_at !== null && <span>{t('providerDetails.expires')} <RelativeTime at={provider.facts.credential.expires_at} /></span>}</p>}
    <div className="flex items-center gap-1"><Button pending={retry.isPending} onClick={() => retry.mutate()}>{t('providerRecovery.checkSignIn')}</Button><HelpHint term={t('providerRecovery.checkSignIn')} what={t('providerLogin.retryHelp')}/></div>
    {retry.error && <ErrorAlert error={retry.error} />}
  </Section>;
}
