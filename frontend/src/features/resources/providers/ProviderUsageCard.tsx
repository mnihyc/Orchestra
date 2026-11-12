/** The source's members and states rendered alike on the page, thread Usage and compact provider choices. */
import { RefreshCw } from 'lucide-react';
import type { ReactNode } from 'react';
import type { TFunction } from 'i18next';
import { Link } from '@tanstack/react-router';
import { ForecastLine } from '@/features/usage/ForecastLine';
import { useTranslation } from 'react-i18next';

import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { formatExact, formatSpan, HOUR_MS, useNow } from '@/lib/time';
import { Button } from '@/ui/Button';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { InlineAlert } from '@/ui/InlineAlert';
import { ErrorAlert } from '@/ui/ErrorState';
import { useErrorText } from '@/ui/errorText';
import { StateLine } from '@/ui/StateLine';
import { Meter } from '@/ui/Meter';
import { RelativeTime } from '@/ui/RelativeTime';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';

import type { PlanProvider, PlanWindow } from './plan';
import { observedAt, privateProvider, resetState,planSectionId } from './plan';
import { amountText, detailFieldVisible, detailState, unavailableReason, scaledAmountText, endpointsNeedNoKey, FACT_MEMBERS, factsSource, reportedList, windowName, type Balance, type DetailMember, type Spend } from './details';

/** Idle sources refresh hourly; an older observation is explicitly stale, never silently zeroed. */
const STALE_AFTER_MS = HOUR_MS;

export function SourceLine({ provider, field }: { provider: PlanProvider; field: string }) {
  const { t } = useTranslation();
  const now = useNow();
  const at = observedAt(provider, field);
  const member=field.split('.')[0];
  const overridden=provider.facts_overridden.includes(field) || member!==undefined && provider.facts_overridden.includes(member);
  return (
    <p className="flex flex-wrap gap-x-3 gap-y-1 text-xs leading-4 text-text-subtle" data-source={field}>
      {overridden && <span data-fact-override>{t('providerCredits.overridden')}</span>}
      {at === null ? t('providerPlan.notReported') : <><span>{t('providerPlan.observed')} <RelativeTime at={at} /></span>{now - Date.parse(at) > STALE_AFTER_MS && <span>{t('providerPlan.stale')}</span>}</>}
      {provider.status === 'offline' && <span>{t('providerPlan.offline')}</span>}
    </p>
  );
}

export function WindowRow({ window, provider }: { window: PlanWindow; provider: PlanProvider }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const now = useNow();
  const state = resetState(window);
  const label = windowName(window);
  const reset = state === 'known' && window.resets_at !== null
    ? t('providerPlan.resetsIn', { time: formatSpan(Math.max(0, Date.parse(window.resets_at) - now), format) })
    : t(state === 'unset' ? 'providerPlan.resetUnset' : 'providerPlan.resetUnknown');
  return (
    <li className="flex min-w-0 flex-col gap-2">
      <Meter neutral showActual value={window.used_percent} label={label} />
      <span className="flex flex-wrap items-baseline justify-between gap-x-2 gap-y-1 text-sm leading-5">
        {window.is_active === true ? <span className="rounded-sm bg-bg-muted px-1 text-xs text-text-muted">{t('providerPlan.binding')}</span> : <span />}
        <span className="ml-auto text-right text-text-muted" title={window.resets_at === null ? undefined : formatExact(window.resets_at, format)}>{reset}</span>
      </span>
      <ForecastLine forecast={provider.plan_forecasts?.[window.id]} usedPercent={window.used_percent} resetsAt={window.resets_at} lastAt={observedAt(provider,`windows.${window.id}`) ?? provider.facts_at} />
      <SourceLine provider={provider} field={`windows.${window.id}`} />
    </li>
  );
}

export interface ProviderUsageCardProps {
  provider?: PlanProvider;
  compact?: boolean;
  /** Inspection cards retain every reported window and its reset; picker captions keep binding windows only. */
  allWindows?: boolean;
  loading?: boolean;
  error?: unknown;
  refreshing?: boolean;
  onRefresh?: () => void;
  actions?: React.ReactNode;
}

const MEMBER_LABELS = {
  identity:'providerDetails.members.identity', credential:'providerDetails.members.credential', credential_verified:'providerDetails.members.credential_verified',
  plan:'providerDetails.members.plan', windows:'providerDetails.members.windows', limit_reached:'providerDetails.members.limit_reached',
  ordinary_usage_allowed:'providerDetails.members.ordinary_usage_allowed', balances:'providerDetails.members.balances', spend:'providerDetails.members.spend',
  extra_usage:'providerDetails.members.extra_usage', available_resets:'providerDetails.members.available_resets',
} as const;
const EMPTY_LABELS = {
  identity:'providerDetails.empty.identity', credential:'providerDetails.empty.credential', credential_verified:'providerDetails.empty.credential_verified',
  plan:'providerDetails.empty.plan', windows:'providerDetails.empty.windows', limit_reached:'providerDetails.empty.limit_reached',
  ordinary_usage_allowed:'providerDetails.empty.ordinary_usage_allowed', balances:'providerDetails.empty.balances', spend:'providerDetails.empty.spend',
  extra_usage:'providerDetails.empty.extra_usage', available_resets:'providerDetails.empty.available_resets',
} as const;
const PERIOD_LABELS = { day:'providerDetails.period.day', week:'providerDetails.period.week', month:'providerDetails.period.month', total:'providerDetails.period.total', window:'providerDetails.period.window' } as const;

function factsSourceTitle(provider: PlanProvider, t: TFunction): string {
  const source = factsSource(provider);
  const noSource = source.by === 'none' || source.by === 'default' && source.name === null && source.descriptor === null;
  return noSource ? t('providerDetails.keeperSource') : source.descriptor?.title ?? t(source.by === 'address' ? 'providerSources.address' : 'providerSources.program');
}

/** Missing members retain their actual source and cause without displaying an older value or clock. */
function MissingMember({ member, provider, showSource = true }: { member: DetailMember; provider: PlanProvider; showSource?: boolean }) {
  const { t } = useTranslation();
  const state = detailState(provider, member);
  const label = state === 'not_read_yet' ? 'providerDetails.notReadOnce'
    : unavailableReason(provider, member) === 'needs_admin_key' ? 'providerSources.needsAdminKey' : 'providerDetails.notSupplied';
  return <p className="text-sm wrap-anywhere text-text-muted"><span>{t(label)}</span>{showSource && <>{' · '}<span data-fact-source>{factsSourceTitle(provider, t)}</span></>}</p>;
}

/** Supplied and unread members retain their rows; unsupported members share the card's summary.
 * Each named group owns a complete definition list so its role cannot interrupt term/definition relationships. */
export function DetailBlock({ member, provider, children, reading = false }: { member: DetailMember; provider: PlanProvider; reading?: boolean; children: ReactNode }) {
  const { t } = useTranslation();
  const state = detailState(provider,member);
  const missing = state === 'unsupported' || state === 'not_read_yet';
  if (privateProvider(provider) || state === 'unsupported') return null;
  return <div role="group" className="min-w-0 border-b border-border py-2 last:border-b-0" aria-label={t(MEMBER_LABELS[member])} aria-busy={state === 'not_read_yet' && reading || undefined} data-fact-member={member} data-fact-state={state}>
    <dl className="grid min-w-0 gap-x-3 gap-y-1 sm:grid-cols-[minmax(7rem,1fr)_minmax(0,3fr)]">
    <dt className="flex flex-wrap items-baseline justify-between gap-2 text-sm font-medium text-text"><span>{t(MEMBER_LABELS[member])}{':'}</span>{(member==='windows' || member==='balances' || member==='spend' || member==='extra_usage') && state==='told' && <Link to="/resources/providers/$id" params={{id:provider.id}} search={{tab:'usage',view:'usage',plan:member==='windows' ? 'current' : 'history',plan_metric:member==='windows' ? 'subscription' : 'credits'}} hash={planSectionId(provider.id)} className="text-sm font-normal text-accent-text hover:underline">{t('planUsage.overTime')}</Link>}</dt>
    <dd className="flex min-w-0 flex-col gap-1 wrap-anywhere">
    {missing ? <MissingMember member={member} provider={provider} showSource={false} /> : state === 'empty' ? <p className="text-sm text-text-muted">{t(EMPTY_LABELS[member])}</p> : children}
    {!missing && member !== 'windows' && member !== 'balances' && member !== 'spend' && <SourceLine provider={provider} field={member} />}
    {state === 'empty' && (member === 'windows' || member === 'balances' || member === 'spend') && <SourceLine provider={provider} field={member} />}
    </dd>
    </dl>
  </div>;
}

function BalanceRow({ balance, provider }: { balance: Balance; provider: PlanProvider }) {
  const { t } = useTranslation();
  const amount = (value: string | null | undefined) => amountText(value,balance.unit,t('providerPlan.notReported'));
  const visible=(field:string)=>detailFieldVisible(provider,`balances.${balance.id}.${field}`);
  const fields={remaining:visible('remaining'),unlimited:visible('unlimited'),total:visible('total'),used:visible('used')};
  return <li className="flex min-w-0 flex-col gap-1 text-sm">
    <h4 className="font-medium">{balance.label ?? t('providerDetails.balanceLabel', { unit: balance.unit })}</h4>
    {(fields.remaining || fields.unlimited && balance.unlimited===true) && <p className="font-medium tabular-nums">{balance.unlimited === true ? t('providerPlan.unlimited') : t('providerDetails.balanceRemaining',{ amount:amount(balance.remaining) })}</p>}
    {fields.total && fields.used ? <p>{t('providerDetails.balanceTotals',{ total:amount(balance.total), used:amount(balance.used) })}</p> : <>{fields.total && <p>{t('providerCredits.total',{amount:amount(balance.total)})}</p>}{fields.used && <p>{t('providerCredits.used',{amount:amount(balance.used)})}</p>}</>}
    {reportedList(balance.parts).length > 0 && <ul>{reportedList(balance.parts).map((part,index) => <li key={index}>{part.label}{': '}{amount(part.amount)}</li>)}</ul>}
    {balance.expires_at !== null && <p>{t('providerDetails.expires')}{' '}<RelativeTime at={balance.expires_at} /></p>}
    <SourceLine provider={provider} field={'balances.' + balance.id} />
  </li>;
}

function SpendRow({ spend, provider }: { spend: Spend; provider: PlanProvider }) {
  const { t } = useTranslation();
  const amount = (value: string | null | undefined) => amountText(value,spend.unit,t('providerPlan.notReported'));
  const visible=(field:string)=>detailFieldVisible(provider,`spend.${spend.id}.${field}`);
  const period = spend.period === 'window' && spend.period_minutes !== null
    ? t('providerDetails.windowPeriod',{ count:spend.period_minutes }) : t(PERIOD_LABELS[spend.period]);
  const fields={used:visible('used'),limit:visible('limit'),remaining:visible('remaining')};
  return <li className="flex min-w-0 flex-col gap-1 text-sm">
    <h4 className="font-medium">{spend.label ?? period}{spend.label !== null && spend.label !== period && ` (${period})`}</h4>
    {fields.used && fields.limit && fields.remaining ? <p className="tabular-nums">{t('providerDetails.spendTotals',{ used:amount(spend.used), limit:amount(spend.limit), remaining:amount(spend.remaining) })}</p> : <>{fields.used && <p>{t('providerCredits.used',{amount:amount(spend.used)})}</p>}{fields.limit && <p>{t('providerCredits.limit',{amount:amount(spend.limit)})}</p>}{fields.remaining && <p>{t('providerDetails.balanceRemaining',{amount:amount(spend.remaining)})}</p>}</>}
    {spend.resets_at !== null && <p>{t('providerDetails.resets')}{' '}<RelativeTime at={spend.resets_at} /></p>}
    <SourceLine provider={provider} field={'spend.' + spend.id} />
  </li>;
}

function ExtraUsageRow({provider}:{provider:PlanProvider}) {
  const {t}=useTranslation();
  const extra=provider.facts.extra_usage;
  if(extra===null)return null;
  const amount=(value:string | null | undefined)=>scaledAmountText(value,extra.unit,extra.decimal_places,t('providerPlan.notReported'),t('providerPlan.unitUnknown'));
  const visible=(field:string)=>detailFieldVisible(provider,`extra_usage.${field}`);
  const fields={used:visible('used'),limit:visible('limit'),disabled_reason:visible('disabled_reason'),used_percent:visible('used_percent')};
  return <div className="flex flex-col gap-2">
    <p className="flex flex-wrap gap-x-3 gap-y-1 text-sm text-text-muted">{!extra.enabled ? <span>{t('providerPlan.extraOff')}</span> : fields.used && fields.limit ? <span>{t('providerPlan.extraAmount',{used:amount(extra.used),limit:amount(extra.limit)})}</span> : <>{fields.used && <span>{t('providerCredits.used',{amount:amount(extra.used)})}</span>}{fields.limit && <span>{t('providerCredits.limit',{amount:amount(extra.limit)})}</span>}</>}{fields.disabled_reason && extra.disabled_reason!==null && <span>{extra.disabled_reason}</span>}</p>
    {fields.used_percent && extra.used_percent!==null && <Meter neutral showActual value={extra.used_percent} label={t('providerPlan.extraSpend')} />}
  </div>;
}

/** Current source facts share one row with the history controls, with each member's own date. */
export function ProviderUsageSummary({provider,windows=provider.facts.windows}:{provider:PlanProvider;windows?:readonly PlanWindow[]}) {
  const {t}=useTranslation();
  const format=useFormatContext();
  const members=['windows','balances','spend','extra_usage'] as const;
  if (privateProvider(provider)) return null;
  return <dl className="grid min-w-0 grid-cols-1 gap-3 sm:grid-cols-2" data-provider-current>
    {members.map((member)=>{
      const state=detailState(provider,member);
      if (state === 'unsupported' || state === 'not_read_yet') return <div key={member} className="min-w-0" data-current-member={member} data-fact-state={state}><dt className="text-sm font-medium text-text">{t(MEMBER_LABELS[member])}</dt><dd><MissingMember member={member} provider={provider} /></dd></div>;
      return <div key={member} className="flex min-w-0 flex-col gap-1" data-current-member={member}>
        <dt className="text-sm font-medium text-text">{t(MEMBER_LABELS[member])}</dt>
        <dd className="min-w-0 text-sm text-text-muted">{state==='empty' ? <p>{t(EMPTY_LABELS[member])}</p> : member==='windows' ? <ul className="flex flex-col gap-2">{windows.map((window)=><li key={window.id}><p>{t('providerPlan.roomUsed',{label:windowName(window),percent:formatNumber(window.used_percent,format.locale,{maximumFractionDigits:2})})}{window.resets_at!==null && <> · {t('providerDetails.resets')} <RelativeTime at={window.resets_at} /></>}</p><SourceLine provider={provider} field={`windows.${window.id}`} /></li>)}</ul> : member==='balances' ? <ul className="flex flex-col gap-2">{provider.facts.balances.map((entry)=><BalanceRow key={entry.id} balance={entry} provider={provider} />)}</ul> : member==='spend' ? <ul className="flex flex-col gap-2">{provider.facts.spend.map((entry)=><SpendRow key={entry.id} spend={entry} provider={provider} />)}</ul> : <ExtraUsageRow provider={provider} />}
        {(state!=='told' || member==='extra_usage') && <SourceLine provider={provider} field={member} />}</dd>
      </div>;
    })}
  </dl>;
}

export function ProviderUsageCard({ provider, compact = false, allWindows = false, loading = false, error, refreshing = false, onRefresh, actions }: ProviderUsageCardProps) {
  const { t } = useTranslation();
  const describe = useErrorText();
  const format = useFormatContext();
  if (loading) return <SkeletonGroup className="flex flex-col gap-4 rounded-lg border border-border bg-surface p-4"><Skeleton className="h-5 w-1/3" /><Skeleton className="h-4 w-full" /><Skeleton className="h-4 w-2/3" /></SkeletonGroup>;
  if (provider === undefined) return <p className="text-sm text-text-muted">{t('providerPlan.notReported')}</p>;
  const administrative = adminAccessOf(provider);
  if (privateProvider(provider)) return compact ? <span className="text-xs text-text-muted">{t('providerPlan.roomHidden')}</span> : null;
  const { facts } = provider;
  const source = factsSource(provider);
  const descriptor = source.descriptor;
  const sourceTitle = factsSourceTitle(provider, t);
  const sourceError = source.last_error;
  const windows = reportedList(facts.windows).filter((window) => Number.isFinite(window.used_percent));
  const binding = windows.filter((window) => window.is_active === true);
  const compactWindows = !allWindows && windows.some((window) => window.is_active !== null) ? binding : windows;
  const balance = detailState(provider,'balances') === 'told' ? reportedList(facts.balances)[0] : undefined;
  const spend = detailState(provider,'spend') === 'told' ? reportedList(facts.spend)[0] : undefined;
  const needsAdmin = FACT_MEMBERS.some((member)=>unavailableReason(provider,member)==='needs_admin_key');
  if (compact) return <span className="flex min-w-0 flex-col gap-2" data-provider-usage="compact">
    <AsAdminMark of={administrative?.place} />
    {detailState(provider,'windows') === 'told' && compactWindows.length > 0 ? <>
      {compactWindows.map((window) => <span key={window.id} className="flex flex-col gap-2"><span className="text-xs text-text-muted">{allWindows ? windowName(window) : t('providerPlan.roomUsed',{ label:windowName(window),percent:formatNumber(window.used_percent,format.locale,{maximumFractionDigits:2}) })}</span><Meter neutral showActual look={allWindows ? 'compact' : 'progress'} value={window.used_percent} label={windowName(window)} />{allWindows && <span className="text-sm text-text-muted">{window.resets_at === null ? t(window.reset_not_started === true ? 'providerPlan.resetUnset' : 'providerPlan.resetUnknown') : <>{t('providerDetails.resets')}{' '}<RelativeTime at={window.resets_at} /></>}</span>}</span>)}
      <SourceLine provider={provider} field="windows" />
    </> : balance !== undefined && balance.remaining !== null ? <>
      <span className="text-xs text-text-muted">{t('providerPlan.roomLeft',{ amount:amountText(balance.remaining,balance.unit,t('providerPlan.notReported')) })}</span><SourceLine provider={provider} field={'balances.' + balance.id} />
    </> : spend !== undefined ? <><span className="text-xs text-text-muted">{t('providerDetails.compactSpend',{ amount:amountText(spend.used,spend.unit,t('providerPlan.notReported')),period:t(PERIOD_LABELS[spend.period]) })}</span><SourceLine provider={provider} field={'spend.' + spend.id} /></> : <span className="text-xs text-text-muted">{t(needsAdmin ? 'providerSources.needsAdminKey' : 'providerPlan.notReported')}</span>}
    {sourceError !== null && <span className="text-xs text-warning">{t('providerDetails.lastReadFailed')}</span>}
  </span>;
  const unread = FACT_MEMBERS.filter((member) => detailState(provider,member) === 'not_read_yet');
  const unsupported = FACT_MEMBERS.filter((member) => detailState(provider,member) === 'unsupported');
  const notSupplied = unsupported.filter((member) => unavailableReason(provider,member) !== 'needs_admin_key');
  const needsAdminMembers = unsupported.filter((member) => unavailableReason(provider,member) === 'needs_admin_key');
  const memberNames = (members: readonly DetailMember[]) => new Intl.ListFormat(format.locale, { type: 'unit' }).format(members.map((member) => t(MEMBER_LABELS[member])));
  const plan = facts.plan;
  const planName = plan === null ? t('providerDetails.empty.plan') : descriptor?.facts?.plans[plan] ?? plan;
  const credential = facts.credential;
  const credentialText = credential?.yields === 'none'
    ? provider.kind === 'api' && endpointsNeedNoKey(provider) ? t('providerDetails.noCredentialNeeded')
      : provider.kind === 'api' && provider.key_set ? t('providerDetails.key') : t('providerDetails.noCredentialHeld')
    : credential?.yields === 'key' ? t('providerDetails.key') : credential?.yields === 'login' ? t('providerDetails.loginShort') : t('providerDetails.tokenShort');
  return <section className="flex min-w-0 flex-col gap-2" aria-label={t('providerDetails.title',{source:sourceTitle})} data-provider-usage="full">
    <div className="flex flex-wrap items-start justify-between gap-2">
      <h2 className="min-w-0 text-base font-semibold text-text">{t('providerDetails.title',{source:sourceTitle})}</h2>
      {onRefresh !== undefined && <Button size="sm" icon={<RefreshCw aria-hidden />} pending={refreshing} onClick={onRefresh}>{t(refreshing ? 'providerPlan.reading' : 'providers.overview.refresh')}</Button>}
    </div>
    {unread.length > 0 && <p data-unread-members role={refreshing ? 'status' : undefined} aria-busy={refreshing} className="text-sm wrap-anywhere text-text-muted">
      {t('providerDetails.notReadOnce')}{': '}{unread.map((member,index) => <span key={member} data-fact-member={member} data-fact-state="not_read_yet">{index > 0 && ', '}{t(MEMBER_LABELS[member])}</span>)}
    </p>}
    {error !== undefined && error !== null && <ErrorAlert error={error} />}
    {sourceError !== null && <StateLine role="alert" tone="warning" compact title={t('providerDetails.lastReadFailed')} message={<>{describe(sourceError).message}{' '}<RelativeTime at={sourceError.at} /></>} />}
    <div className="min-w-0">
    <DetailBlock member="identity" provider={provider} reading={refreshing}><p className="break-words text-sm text-text-muted">{facts.identity ?? t('providerDetails.empty.identity')}</p></DetailBlock>
    <DetailBlock member="credential" provider={provider} reading={refreshing}>{credential !== null && <div className="flex flex-col gap-1 text-sm text-text-muted">
      <p>{credentialText}</p>
      {credential.expires_at !== null && <p>{t('providerPlan.lapses',{when:formatExact(credential.expires_at,format)})}</p>}
      {credential.yields === 'token' && credential.expires_at === null && <p>{t('providerPlan.lapseUnknown')}</p>}
      {credential.issued_at !== null && <p>{t('providerPlan.issued',{when:formatExact(credential.issued_at,format)})}</p>}
    </div>}</DetailBlock>
    <DetailBlock member="credential_verified" provider={provider} reading={refreshing}><p className="text-sm text-text-muted">{t(facts.credential_verified === true ? 'providerDetails.verifiedShort' : 'providerDetails.notVerifiedShort')}</p></DetailBlock>
    <DetailBlock member="plan" provider={provider} reading={refreshing}><p className="text-sm font-medium">{planName}</p></DetailBlock>
    <DetailBlock member="windows" provider={provider} reading={refreshing}><ul className="flex flex-col gap-4">{windows.map((window) => <WindowRow key={window.id} window={window} provider={provider} />)}</ul></DetailBlock>
    <DetailBlock member="limit_reached" provider={provider} reading={refreshing}>{facts.limit_reached !== null && <InlineAlert tone="warning"><p>{facts.limit_reached.type}</p>{facts.limit_reached.resets_at !== null && <p>{t('providerDetails.resets')}{' '}<RelativeTime at={facts.limit_reached.resets_at} /></p>}</InlineAlert>}</DetailBlock>
    <DetailBlock member="ordinary_usage_allowed" provider={provider} reading={refreshing}><p className="text-sm text-text-muted">{t(facts.ordinary_usage_allowed === false ? 'providerDetails.refusedShort' : 'providerDetails.allowedShort')}</p></DetailBlock>
    <DetailBlock member="balances" provider={provider} reading={refreshing}><ul className="flex flex-col gap-3">{reportedList(facts.balances).map((entry) => <BalanceRow key={entry.id} balance={entry} provider={provider} />)}</ul></DetailBlock>
    <DetailBlock member="spend" provider={provider} reading={refreshing}><ul className="flex flex-col gap-3">{reportedList(facts.spend).map((entry) => <SpendRow key={entry.id} spend={entry} provider={provider} />)}</ul></DetailBlock>
    <DetailBlock member="extra_usage" provider={provider} reading={refreshing}><ExtraUsageRow provider={provider} /></DetailBlock>
    <DetailBlock member="available_resets" provider={provider} reading={refreshing}>{facts.available_resets !== null && <><p className="text-sm text-text-muted">{t('providerPlan.resetsAvailable',{count:facts.available_resets.available_count})}</p><ul className="flex flex-col gap-2">{reportedList(facts.available_resets.credits).map((credit,index) => <li key={credit.id}><p>{credit.title ?? t('providerDetails.resetNumber', { number: index + 1 })}</p>{credit.description !== null && <p className="text-sm text-text-muted">{credit.description}</p>}{credit.expires_at !== null && <RelativeTime at={credit.expires_at} />}</li>)}</ul></>}</DetailBlock>
    </div>
    {unsupported.length > 0 && <p data-unsupported-members className="text-sm wrap-anywhere text-text-muted">
      {notSupplied.length > 0 && <span>{t('providerDetails.notSuppliedMembers',{members:memberNames(notSupplied)})}</span>}
      {notSupplied.length > 0 && needsAdminMembers.length > 0 && ' · '}
      {needsAdminMembers.length > 0 && <span data-needs-admin-members>{t('providerDetails.needsAdminKeyMembers',{members:memberNames(needsAdminMembers)})}</span>}
    </p>}
    {actions}
  </section>;
}

