/** Every holder, human effect, agent reach and policy source in an ordinary share preview. */
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { ApiError } from '@/api/client';
import type { AccessTarget } from '@/api/access';
import { Account } from '@/features/access/ChangePreview';
import { AREA_WORDS, stepWords } from '@/features/access/AgentRow';
import { AREAS, stepOf } from '@/features/access/agentShares';
import { HolderView } from '@/features/access/holders';
import { usePermissionName } from '@/features/access/names';
import { optionLabel } from '@/features/access/EntryOptions';
import { SettingReadout } from '@/features/settings/SettingReadout';
import { offeredChoice } from '@/i18n/choices';
import { ReviewPlace } from '@/features/project/ReviewConsequences';
import type { Entry, SharePreview } from '@/generated/api-types';
import { ErrorAlert } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';
import { UserChip } from '@/ui/UserChip';
import { ValueSummary } from '@/ui/ValueSummary';

function EntryFacts({ entries, target }: { entries: readonly Entry[]; target: AccessTarget }) {
  const { t } = useTranslation();
  const word=(text:string)=>offeredChoice(t,'arrangement',text)?.name??offeredChoice(t,'effort',text)?.name??text;
  return entries.length === 0 ? <EmptyState compact title={t('shareReview.noEntries')} /> : <ul className="flex flex-col gap-2">{entries.map((entry) => <li key={entry.id} className="flex flex-col gap-1">
    <HolderView holder={entry.holder} target={target} />
    <p>{entry.expires_at === null ? t('shareReview.noExpiry') : <RelativeTime at={entry.expires_at} />}</p>
    {entry.note !== '' && <p className="wrap-anywhere">{entry.note}</p>}
    {entry.read_first && <p>{t('people.readFirst')}</p>}
    {entry.options !== null && <ValueSummary value={entry.options} label={t('shareReview.options')} fieldName={(name) => optionLabel(t, name)} word={word} />}
  </li>)}</ul>;
}

export function ShareConsequences({ preview, target }: { preview: SharePreview; target: AccessTarget }) {
  const { t } = useTranslation();
  const permissionName = usePermissionName();
  return <section className="flex flex-col gap-3" aria-label={t('shareReview.consequences')}>
    <p>{preview.permissions.map(permissionName).join(', ')}</p>
    <ul className="flex flex-wrap gap-2">{preview.holders.map((holder, index) => <li key={index}><HolderView holder={holder} target={target} /></li>)}</ul>
    <Account preview={preview.effects} />
    {preview.already.length > 0 && <section><h4>{t('shareReview.already')}</h4><ul>{preview.already.map((person) => <li key={person.user.id} className="flex flex-wrap gap-2"><UserChip user={person.user} size={24} /><span>{person.permissions.map(permissionName).join(', ')}</span></li>)}</ul></section>}
    {preview.already_more > 0 && <p>{t('shareReview.alreadyMore', { count: preview.already_more })}</p>}
    <ul className="flex flex-col gap-3">{preview.rows.map((row) => <li key={`${String(row.defaults)}:${row.permission}:${row.profile}`} className="rounded border border-border p-2">
      <h4 className="font-medium">{permissionName(row.permission)}</h4><p className="text-text-muted">{t(row.defaults ? 'shareReview.defaults' : 'shareReview.ownPage')}</p>{row.profile !== 'any' && <p>{t(row.profile === 'managed' ? 'trace.profiles.managed' : 'trace.profiles.connected')}</p>}
      <h5>{t('shareReview.currentEntries')}</h5><EntryFacts entries={row.before} target={target} />
      <h5>{t('shareReview.proposedEntries')}</h5><EntryFacts entries={row.after} target={target} />
    </li>)}</ul>
    <ul className="flex flex-col gap-2">{preview.agents.map((agent, index) => <li key={index} className="rounded border border-border p-2">
      <ReviewPlace place={agent.to} /><p>{t(agent.inside ? 'shareReview.inside' : 'shareReview.itself')}</p>
      <ul>{AREAS.map((area) => <li key={area}>{t(AREA_WORDS[area])}{': '}{stepWords(t, area, stepOf(area, agent.already_shared))}{' → '}{stepWords(t, area, stepOf(area, agent.parts))}</li>)}</ul>
      <p>{t(agent.download ? 'shareReview.download' : 'shareReview.noDownload')}</p>
      {agent.future_threads && <p>{t('shareReview.future')}</p>}
      {agent.copy !== null && <p>{t('shareReview.copy', { count: agent.copy.files, bytes: agent.copy.bytes })}</p>}
      {agent.copy?.already_sent === true && <p>{t('shareReview.copyAlready')}</p>}
    </li>)}</ul>
    <section><h4>{t('shareReview.rules')}</h4><ul className="flex flex-col gap-2">{preview.rules.map((rule) => <li key={rule.key}>
      <SettingReadout settingKey={rule.key} value={rule.value} />
      <div className="flex flex-wrap gap-x-3 gap-y-1 text-text-muted"><span>{t(rule.sharing ? 'shareReview.sharingRule' : 'shareReview.laterRule')}</span><span>{rule.source === null ? t('shareReview.default') : rule.source === 'system' ? t('shareReview.system') : <ReviewPlace place={rule.source} />}</span></div>
    </li>)}</ul></section>
    {preview.refusal !== null && <ErrorAlert error={new ApiError(preview.refusal, null)} />}
    {!preview.allowed && preview.refusal === null && <p>{t('shareReview.notAllowed')}</p>}
  </section>;
}
