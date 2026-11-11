/** One share with agents as a line of the Agents tab: whose agents, at what level in each area, until when, who shared it and since when, the level changed after being asked about, and the ✕ that takes it back.
 *
 * A live share is changed area by area; a copy is only taken back, which removes its files from the agent's
 * workspace. The source page’s managers, and its giver while they could still give it again, may take back a live share:
 * for anybody else the controls stand off and say so. Every change says what it does before it is made.
 */
import { useMutation } from '@tanstack/react-query';
import { CalendarClock, X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { AccessTarget } from '@/api/access';
import { changeAgentShare, stopAgentShare } from '@/api/share';
import type { AgentShare, PermissionPage, ShareParts } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { Button } from '@/ui/Button';
import { ByteSize } from '@/ui/ByteSize';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { DateField } from '@/ui/DateField';
import { Select } from '@/ui/Select';

import { AgentAdvancedRows } from './AgentAdvancedRows';
import { ADVANCED, hasAgentParts, type Area, areasOf, isCopy, mayGiveParts, reachOf, stepOf, stepsOf, withStep } from './agentShares';
import { AgentPartsSummary, CurrentAgentContribution, ViewerReadStanding } from './AgentContribution';
import { shortDate } from './holders';

import { AREA_WORDS, stepOption, stepWords } from './agentShareWords';
export { AREA_WORDS, stepOption, stepWords } from './agentShareWords';

interface Change {
  area?: Area;
  parts: ShareParts;
}

export function AgentRow({ share, target, available, page, meId, remaining = [] }: { share: AgentShare; target: AccessTarget; available: ShareParts; page: PermissionPage | undefined; meId: string | undefined; remaining?: readonly AgentShare[] }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const copy = isCopy(share);
  const reach = reachOf(share);
  const areas = areasOf(target.kind, reach);
  const mine = meId !== undefined && share.by?.id === meId;
  const affected = page?.rows.filter((row) => row.entries.some((entry) => share.entries.some((given) => given.id === entry.id))) ?? [];
  const mayGive = mayGiveParts(share.parts, available) && affected.every((row) => row.can_give);
  const mineAndGiveable = mine && mayGive;
  const managesEntries = share.entries.length > 0 && share.entries.every((given) => affected.some((row) => row.writable && row.entries.some((entry) => entry.id === given.id)));
  const managesLoan = !copy && !affected.some((row) => row.locked !== null) && (page?.owners_writable === true || managesEntries);
  const may = mineAndGiveable || managesLoan;
  const mayRemove = may;
  const why = may ? undefined : t('access.agents.notYoursToChange');
  const [asking, setAsking] = useState<Change | null>(null);
  const [removing, setRemoving] = useState(false);
  const otherShares = removing ? remaining.filter((other) => other.id !== share.id) : [];
  const [ending, setEnding] = useState(false);
  const [until, setUntil] = useState('');
  const place = share.to.name ?? t('access.place.hidden');
  const who = t('access.holder.agentsOf', { place });
  // In a sentence they are not a name: "Change what the agents of Web may do?"
  const they = t('access.agents.whoIs', { place });

  // What a share's change reaches (the shares listed, the thread's shared list, the page) is the registry of acts' to ask again.
  const change = useMutation({
    mutationFn: (body: { parts?: ShareParts; expires_at?: string | null }) => changeAgentShare(share.id, body),
    onSuccess: () => {
      setAsking(null);
      setEnding(false);
    },
  });
  const stop = useMutation({
    mutationFn: () => stopAgentShare(share.id),
    onSuccess: () => setRemoving(false),
  });

  const onlyArea = areas.filter((area) => stepOf(area, share.parts) > 0).length + ADVANCED.filter((row) => share.parts[row] === true).length <= 1;
  const date = shortDate(locale, share.at);
  const expires = share.snapshot !== null ? null : (share.entries[0]?.expires_at ?? null);

  return (
    <li className="flex flex-col gap-2 px-3 py-3">
      <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
        <div className="flex min-w-0 flex-wrap items-baseline gap-x-2">
          <span className="font-medium wrap-anywhere text-text">{who}</span>
          {target.kind !== 'thread' && !copy && reach === 'alone' && <span className="text-sm text-text-muted">· {t('access.agents.placeAlone', { name: target.name })}</span>}
          {copy && (
            <span className="text-sm text-text-muted">
              {t('access.agents.copy', { date, state: t(`access.agents.state.${share.snapshot?.state ?? 'delivered'}`) })}
              {share.snapshot !== null && (
                <>
                  {' · '}
                  <ByteSize bytes={share.snapshot.bytes} />
                </>
              )}
            </span>
          )}
        </div>
        <div className="flex items-center gap-1">
          {!copy && (
            <Button size="sm" variant="ghost" icon={<CalendarClock aria-hidden />} blocked={!may} title={why} onClick={() => { setUntil(expires === null ? '' : expires.slice(0, 10)); setEnding(true); }}>
              {expires === null ? t('access.agents.expiresNever') : t('access.agents.expiresOn', { date: shortDate(locale, expires) })}
            </Button>
          )}
          <IconButton
            size="sm"
            label={copy ? t('access.agents.takeCopyBack', { name: share.to.name ?? '' }) : t('access.agents.stopFor', { name: share.to.name ?? '' })}
            icon={<X aria-hidden />}
            disabled={!mayRemove}
            onClick={() => setRemoving(true)}
          />
        </div>
      </div>

      {!copy && <p className="text-sm font-medium text-text">{t('agentContributions.requested')}</p>}
      {!copy && (
        <div role="group" aria-label={who} className="flex flex-wrap gap-x-4 gap-y-2">
          {areas.map((area) => (
            <Select<string>
              key={area}
              label={t(AREA_WORDS[area])}
              size="sm"
              value={String(stepOf(area, share.parts))}
              disabled={!may || change.isPending}
              fieldClassName="min-w-[9rem] flex-[1_1_9rem]"
              onValueChange={(next) => {
                const step = Number(next);
                if (step !== stepOf(area, share.parts)) setAsking({ area, parts: withStep(share.parts, area, step) });
              }}
              options={stepsOf(area).map((step) => {
                const off = step > stepOf(area, available) || (step === 0 && onlyArea);
                return { value: String(step), ...stepOption(t, area, step), disabled: off };
              })}
            />
          ))}
        </div>
      )}

      {!copy && areas.includes('timeline') && <AgentAdvancedRows parts={share.parts} available={available} disabled={!may || change.isPending}
        onChange={(parts) => { if (hasAgentParts(parts)) setAsking({ parts }); }} />}
      {!copy && <CurrentAgentContribution current={share.current} />}
      <ViewerReadStanding entries={share.viewer_entries} />
      <p className="text-sm text-text-muted">{t('access.agents.by', { name: (share.by?.display_name ?? '').length > 0 ? share.by?.display_name : t('agentContributions.giverUnavailable'), date })}</p>

      <ConfirmDialog
        open={asking !== null}
        onOpenChange={(next) => !next && !change.isPending && setAsking(null)}
        title={t('access.agents.change.title', { who: they })}
        confirmLabel={t('access.agents.change.confirm', { who: they })}
        pending={change.isPending}
        error={change.error}
        onConfirm={() => asking !== null && change.mutate({ parts: asking.parts })}
      >
        {asking !== null && (
          <div className="flex flex-col gap-2">
            <div>
              {asking.area === undefined ? <AgentPartsSummary parts={asking.parts} /> : t('access.agents.change.line', {
                area: t(AREA_WORDS[asking.area]),
                from: stepWords(t, asking.area, stepOf(asking.area, share.parts)),
                to: stepWords(t, asking.area, stepOf(asking.area, asking.parts)),
              })}
            </div>
            {asking.area === 'documents' && stepOf(asking.area, asking.parts) === 2 && target.kind !== 'document' && target.kind !== 'thread' && reach === 'inside' && (
              <p className="text-text-muted">{t('access.agents.makeNote')}</p>
            )}
          </div>
        )}
      </ConfirmDialog>

      <ConfirmDialog
        open={removing}
        onOpenChange={(next) => !next && !stop.isPending && setRemoving(false)}
        title={copy ? t('access.agents.takeBack.title', { name: share.to.name ?? '' }) : t('access.agents.stop.title', { who: they })}
        confirmLabel={copy ? t('access.agents.takeBack.confirm') : t('access.agents.stop.confirm')}
        danger
        pending={stop.isPending}
        error={stop.error}
        onConfirm={() => stop.mutate()}
      >
        {copy ? t('access.agents.takeBack.body', { name: share.to.name ?? '' }) : <div className="flex flex-col gap-2">
          <p>{t('access.agents.stop.body')}</p><ViewerReadStanding entries={share.viewer_entries} />
          {otherShares.length > 0 && <><p className="font-medium">{t('agentContributions.remaining')}</p><ul className="flex flex-col gap-2">{otherShares.map((other) => <li key={other.id}>
            <p>{t('agentContributions.recordedFor', { giver: (other.by?.display_name ?? '').length > 0 ? other.by?.display_name : t('agentContributions.giverUnavailable'), recipient: other.to.name ?? t('access.place.hidden') })}</p>
            <p className="text-xs text-text-muted">{t('agentContributions.requested')}</p><AgentPartsSummary parts={other.parts} />
          </li>)}</ul></>}
        </div>}
      </ConfirmDialog>

      <Dialog
        open={ending}
        onOpenChange={(next) => !next && !change.isPending && setEnding(false)}
        size="sm"
        title={t('access.agents.expiresTitle', { who: they })}
        footer={
          <>
            <Button disabled={change.isPending} onClick={() => setEnding(false)}>
              {t('common.cancel')}
            </Button>
            <Button variant="primary" pending={change.isPending} onClick={() => change.mutate({ expires_at: until === '' ? null : new Date(`${until}T23:59:59`).toISOString() })}>
              {t('access.roles.endSave')}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-3 py-2">
          <DateField label={t('access.agents.expiresLabel')} value={until} onValueChange={setUntil} hint={t('access.roles.endHint')} />
          {change.error !== null && <ErrorAlert error={change.error} />}
        </div>
      </Dialog>
    </li>
  );
}
