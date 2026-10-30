/** The provider's Details section keeps drafts against the reviewed revision and never reads back a source secret. */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { machinePolicyQuery, providerSetupQuery, setProviderSources } from '@/api/resources';
import { settingsQuery } from '@/api/settings';
import { ProviderGuideLink } from '@/features/help/ProviderGuideLink';
import { SettingHelpLink } from '@/features/help/SettingHint';
import { WhereSet } from '@/features/help/WhereSet';
import type { Field, Provider, ProviderSourcesSetBody, SourceChoice } from '@/generated/api-types';
import { holds, denial } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { adminSpec } from '@/ui/AdminAct';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { useErrorText } from '@/ui/errorText';
import { StateLine } from '@/ui/StateLine';
import type { FormValue, FormValues } from '@/ui/FieldForm';
import { InlineAlert } from '@/ui/InlineAlert';
import { HelpHint } from '@/ui/HelpHint';
import { RelativeTime } from '@/ui/RelativeTime';
import { Section } from '@/shell/Page';
import { SkeletonRows } from '@/ui/Skeleton';
import { linkClasses } from '@/ui/link';
import { emptySourceDraft, secretSourceField, SourceFields, sourceFault, sourceValues, type SourceDraft, type SourceFieldsProps, type SourceProgram } from './SourceFields';
import { defaultOffer, programOffers, selectedProgram, sourceChoiceSet } from './sourceOptions';

/** A saved read failure keeps its human explanation and observation time in the common status line. */
function SourceFailure({ failure }: { failure: { code: string; message: string; at: string } }) {
  const { t } = useTranslation();
  const text = useErrorText()(failure);
  return <StateLine role="alert" tone="warning" compact title={t('providerSources.savedRead')}
    message={<>{text.message}{' '}<RelativeTime at={failure.at} /></>} />;
}

/** The setup response projected for one editor, not a second source-selection wire type. */
export interface SourceOffers { defaultTitle:string; defaultSummary:string; programs:readonly SourceProgram[] }
export interface SourcesChange { facts:SourceDraft | null; models:SourceDraft | null }
export interface SourcesFormProps {
  provider:Provider;
  facts:SourceOffers;
  models:SourceOffers;
  httpAllowed:boolean | null;
  onSave:(change:SourcesChange,reviewed:Provider) => Promise<Provider>;
  warnings?:(element:'facts' | 'models',address:string) => readonly ReactNode[];
  unavailable?:boolean;
}

function formValue(value:unknown):value is FormValue {
  return typeof value === 'string' || typeof value === 'boolean' || typeof value === 'object' && value !== null && !Array.isArray(value) && Object.values(value).every((item) => typeof item === 'string');
}
/** Only public config seeds the draft. Stored field names or a declaration's secret default never supply a value. */
export function sourceDraft(choice:SourceChoice | null | undefined):SourceDraft {
  if (choice === null || choice === undefined) return emptySourceDraft();
  const fields=choice.descriptor?.fields ?? [];
  const secrets=new Set(fields.filter(secretSourceField).map((field) => field.name));
  const config:FormValues={};
  for (const [name,value] of Object.entries(choice.config)) if (!secrets.has(name) && formValue(value)) config[name]=value;
  return { by:choice.by === 'builtin' ? 'default' : choice.by,program:choice.name ?? '',url:choice.address?.url ?? '',auth:choice.address?.auth ?? 'none',header:choice.address?.header ?? '',authorization:'',sendCredential:choice.address?.send_credential ?? false,fields:sourceValues(fields,config) };
}
export function savedSourceAddress(choice:SourceChoice | null | undefined):SourceFieldsProps['savedAddress'] {
  const address=choice?.address;
  return address === null || address === undefined ? null : {url:address.url,auth:address.auth,header:address.header ?? ''};
}
export function sourceFieldsFor(value:SourceDraft,choice:SourceChoice | null | undefined,offers:SourceOffers):readonly Field[] {
  if (value.by === 'program') return selectedProgram(value,offers.programs)?.fields ?? [];
  return value.by === 'address' && choice?.address?.url === value.url.trim() ? choice.descriptor?.fields ?? [] : [];
}

export function providerSourcesBody(change:SourcesChange,reviewed:Provider,facts:SourceOffers,models:SourceOffers):ProviderSourcesSetBody {
  const body:ProviderSourcesSetBody={expected_revision:reviewed.revision};
  if (change.facts !== null) body.facts=sourceChoiceSet(change.facts,sourceFieldsFor(change.facts,reviewed.sources.facts,facts),facts.programs);
  if (change.models !== null) body.models=sourceChoiceSet(change.models,sourceFieldsFor(change.models,reviewed.sources.models,models),models.programs);
  return body;
}

/** Advisory matches mirror network host entries; the keeper remains the authority for an actual request. */
export function sourceHostAllowed(host:string,allowed:readonly string[]):boolean {
  const name=host.toLowerCase();
  return allowed.some((entry) => {const value=entry.toLowerCase();return value === name || value.startsWith('*.') && name.endsWith(value.slice(1)) && name !== value.slice(2);});
}

/** The existing-provider editor uses the served setup, current template and keeping machine, never all machines' programs. */
export function ProviderSources({provider}:{provider:Provider}) {
  const {t}=useTranslation();
  const setup=useQuery({...providerSetupQuery(provider.home.id,provider.id),retry:false});
  const homeSettings=useQuery({...settingsQuery(provider.home.id),retry:false});
  const keeper=provider.credential_keeper ?? null;
  const policy=useQuery({...machinePolicyQuery(keeper?.id ?? ''),enabled:keeper !== null,retry:false});
  // This setup is scoped by provider_id, so its template is the provider's selected one, even for a manage grantee.
  const unit=provider.machine===null || provider.runtime_support?.lifecycle==='unit';
  const template=unit && (keeper===null || keeper.id===provider.machine?.id) ? setup.data?.runtime_templates[0]?.id ?? provider.runtime?.template.id ?? null : null;
  const declared=setup.data?.sources ?? [];
  const runtimeMachine=unit ? provider.machine?.id : undefined;
  const offers=declared.map((offer) => {
    if (runtimeMachine === undefined || offer.template?.id !== template || offer.descriptor !== null) return offer;
    // Keep the template's registration/command, with what its running program has since described.
    const reported=declared.find((program) => program.by === 'program' && program.name === offer.name && program.machine?.id === runtimeMachine)?.descriptor;
    return reported === null || reported === undefined ? offer : {...offer,descriptor:reported};
  });
  const forElement=(element:'facts' | 'models'):SourceOffers => {
    const current=provider.sources[element];
    const offered=defaultOffer(offers,element,provider.upstreams,provider.agent !== null);
    const descriptor=current.by === 'default' ? current.descriptor : offered?.descriptor;
    const programs=programOffers(offers,element,keeper?.id ?? null,template).map((program) => current.by === 'program' && current.name === program.name && current.descriptor !== null ? {...program,title:current.descriptor.title,summary:current.descriptor.summary,fields:current.descriptor.fields,reports:element === 'facts' ? current.descriptor.facts?.reports ?? [] : []} : program);
    return {defaultTitle:descriptor?.title ?? t(element === 'facts' ? 'providerSources.nothing' : 'providerDetails.defaultSource'),defaultSummary:descriptor?.summary ?? t(element === 'facts' ? 'providerSources.nothingAbout' : 'providerSources.defaultModels'),programs};
  };
  const facts=forElement('facts'),models=forElement('models');
  const httpPolicy=homeSettings.data?.find((setting) => setting.key === 'providers.allow_http_upstreams');
  const warnings=(_element:'facts' | 'models',address:string):ReactNode[] => {
    let url:URL;
    try {url=new URL(address);} catch {return [];}
    const result:ReactNode[]=[];
    const network=provider.runtime?.network;
    if (network !== undefined && (network.mode === 'restricted' || network.required_mode === 'restricted') && !sourceHostAllowed(url.hostname,network.allowed_hosts)) {
      result.push(<span key="network">{t('providerSources.networkWarning',{host:url.host})}{' '}<SettingHelpLink settingKey="network.allowed_hosts" target={{kind:'provider',id:provider.id}} /></span>);
    }
    if (keeper !== null) {
      const upstreams=policy.data?.keys.find((entry) => entry.key === 'provider.upstreams')?.value;
      const allowed=Array.isArray(upstreams) ? upstreams.filter((value):value is string => typeof value === 'string') : null;
      if (!allowed?.includes(address.trim())) result.push(<span key="machine">{t(allowed === null ? 'providerSources.upstreamUnknown' : 'providerSources.upstreamWarning',{address})}{' '}<Link to="/resources/machines/$id" params={{id:keeper.id}} search={{tab:'policy'}} className={linkClasses}>{t('providerSources.machinePolicy',{machine:keeper.name})}</Link></span>);
    }
    return result;
  };
  const save=(change:SourcesChange,reviewed:Provider) => setProviderSources(provider.id,providerSourcesBody(change,reviewed,facts,models));
  if (setup.isPending) return <SkeletonRows rows={3} />;
  return <div className="flex min-w-0 flex-col gap-3">
    {setup.isError && <ErrorState compact error={setup.error} retrying={setup.isRefetching} onRetry={() => void setup.refetch()} />}
    <SourcesForm key={provider.id} provider={provider} facts={facts} models={models} httpAllowed={setup.data?.http_allowed ?? null} onSave={save} warnings={warnings} />
    {setup.data?.http_allowed === false && httpPolicy !== undefined && <WhereSet setting={httpPolicy} def={{key:httpPolicy.key,class:'policy'}} target={{kind:'node',id:provider.home.id}} />}
  </div>;
}

export function SourcesForm({provider,facts,models,httpAllowed,onSave,warnings,unavailable=false}:SourcesFormProps) {
  const {t}=useTranslation();
  const [reviewed,setReviewed]=useState(provider);
  const [details,setDetails]=useState(() => sourceDraft(provider.sources.facts));
  const [listing,setListing]=useState(() => sourceDraft(provider.sources.models));
  const [dirty,setDirty]=useState({facts:false,models:false});
  const [pending,setPending]=useState(false);
  const [error,setError]=useState<unknown>(null);
  const [saved,setSaved]=useState(false);
  const allowed=holds(provider.access,'provider.manage');
  const why=denial(t,'provider.manage',provider.name,provider.owner);
  const changed=reviewed.revision !== provider.revision;
  const context=(element:'facts' | 'models',offers:SourceOffers) => {
    const draft=element === 'facts' ? details : listing;
    const choice=reviewed.sources[element];
    const same=draft.by === choice.by && (draft.by !== 'program' || draft.program === choice.name) && (draft.by !== 'address' || draft.url.trim() === choice.address?.url);
    return {programs:offers.programs,httpAllowed,canSendCredential:provider.kind === 'api',savedAddress:savedSourceAddress(choice),authorizationSet:reviewed.secrets_set.includes('source:' + element + ':authorization'),secretFieldsSet:same ? choice.secrets : [],addressFields:sourceFieldsFor(draft,choice,offers)};
  };
  const factsContext=context('facts',facts),modelsContext=context('models',models);
  const blocked=unavailable || !allowed || changed || !dirty.facts && !dirty.models || dirty.facts && sourceFault(details,factsContext) !== null || dirty.models && sourceFault(listing,modelsContext) !== null;
  const accept=(fresh:Provider) => {setReviewed(fresh);setDetails(sourceDraft(fresh.sources.facts));setListing(sourceDraft(fresh.sources.models));setDirty({facts:false,models:false});setError(null);};
  const save=async () => {
    if (blocked || pending) return;
    setPending(true);setError(null);setSaved(false);
    try {accept(await onSave({facts:dirty.facts ? details : null,models:dirty.models ? listing : null},reviewed));setSaved(true);}
    catch (failure) {setError(failure);}
    finally {setPending(false);}
  };
  const metadata=(element:'facts' | 'models') => {
    const source=provider.sources[element];
    const descriptor=source.descriptor;
    const draft=element === 'facts' ? details : listing;
    const offered=element === 'facts' ? facts : models;
    const namedProgram=source.by === 'program' && draft.by === 'program'
      ? offered.programs.find((one)=>(one.choice ?? one.name) === draft.program && one.name === source.name) : undefined;
    const namedTitle=namedProgram?.title ?? (source.by === 'default' && draft.by === 'default' ? offered.defaultTitle : null);
    // The current selector already names that source; retain reported differences and read times.
    const title=descriptor === null || descriptor.title === namedTitle ? null : descriptor.title;
    const members=descriptor?.facts?.reports.map((member)=>t('providerDetails.members.' + member,{defaultValue:member})).join(', ') ?? '';
    return <div className="flex min-w-0 flex-col gap-2 text-sm text-text-muted">
      {descriptor !== null && <>{title !== null && <p className="font-medium">{title}</p>}{descriptor.summary !== '' && <p>{descriptor.summary}</p>}{namedProgram === undefined && element === 'facts' && descriptor.facts !== null && <p>{t('providerSources.reports',{members})}</p>}{namedProgram === undefined && element === 'models' && descriptor.models !== null && <p>{t('providerSources.listsModels')}</p>}</>}
      {source.described_at !== null && <p>{t('providerSources.described')}{' '}<RelativeTime at={source.described_at} /></p>}
      {source.last_error !== null && <SourceFailure failure={source.last_error} />}
    </div>;
  };
  return <Section id="provider-sources" title={t('providerSources.title')} action={<HelpHint term={t('providerSources.title')}
    what={<div className="flex flex-col gap-2"><p>{t('providerSources.creationLead')}</p><ProviderGuideLink guide="provider-details"/><ProviderGuideLink guide="provider-readme"/><ProviderGuideLink guide="provider-facts"/><ProviderGuideLink guide="provider-models"/></div>}/> }>
    <div className="flex min-w-0 flex-col gap-6">
      {!allowed && <InlineAlert tone="warning">{why}</InlineAlert>}
      {changed && !pending && <InlineAlert tone="warning" action={<Button size="sm" disabled={pending} onClick={() => accept(provider)}>{t('providerSources.useCurrent')}</Button>}>{t('providerSources.stale')}</InlineAlert>}
      <SourceFields element="facts" value={details} onChange={(draft) => {setDetails(draft);setDirty((before) => ({...before,facts:true}));setSaved(false);}} {...factsContext} defaultTitle={facts.defaultTitle} defaultSummary={facts.defaultSummary} disabled={pending || !allowed || unavailable} warnings={warnings?.('facts',details.url) ?? []} />
      {metadata('facts')}
      <SourceFields element="models" value={listing} onChange={(draft) => {setListing(draft);setDirty((before) => ({...before,models:true}));setSaved(false);}} {...modelsContext} defaultTitle={models.defaultTitle} defaultSummary={models.defaultSummary} disabled={pending || !allowed || unavailable} warnings={warnings?.('models',listing.url) ?? []} />
      {metadata('models')}
      {error !== null && <ErrorAlert error={error} />}
      {saved && <p role="status" className="text-sm text-text-muted">{t('providerSources.saved')}</p>}
      <div className="flex flex-wrap gap-2"><Button admin={adminSpec(provider, 'provider.manage')} variant="primary" pending={pending} disabled={blocked} onClick={() => void save()}>{t('providerSources.save')}</Button><SettingHelpLink settingKey="providers.allow_http_upstreams" target={{kind:'node',id:provider.home.id}} /></div>
    </div>
  </Section>;
}
