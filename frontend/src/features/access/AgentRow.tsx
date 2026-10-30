/** One share with agents as a line of the Agents tab: whose agents, at what level in each area, until when, who shared it and since when, the level changed after being asked about, and the ✕ that takes it back.
 *
 * A live share is changed area by area; a copy is only taken back, which removes its files from the agent's
 * workspace. Whoever shared it, and whoever could share the same thing now, may change or take back a share:
 * for anybody else the controls stand off and say so. Every change says what it does before it is made.
 */
import { useMutation } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { CalendarClock, X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import type { AccessTarget } from '@/api/access';
import { changeAgentShare, stopAgentShare } from '@/api/share';
import type { AgentLevel, AgentShare, ShareParts } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { Button } from '@/ui/Button';
import { ByteSize } from '@/ui/ByteSize';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { DateField } from '@/ui/DateField';
import { Select } from '@/ui/Select';

import { type Area, areasOf, isCopy, levelNeeded, mayGiveParts, reachOf, stepOf, stepsOf, withStep } from './agentShares';
import { shortDate } from './holders';

/** What a level is called in an area, in the words of the form: conversations and chats are read, or read and written; documents are read, read and edited, or managed. */
export function stepWords(t: TFunction, area: Area, step: number): string {
  if (step === 0) return t('access.agents.step.none');
  if (step === 1) return t('access.agents.step.read');
  if (step === 2) {
    if (area === 'documents') return t('access.agents.step.edit');
    return t('access.agents.step.write');
  }
  return t('access.agents.step.manage');
}

/** The short choice name, with the existing area-specific wording kept as its description. */
export function stepOption(t: TFunction, area: Area, step: number) {
  const word = step === 0 ? 'none' : step === 1 ? 'view' : step === 2 ? area === 'documents' ? 'edit' : 'write' : 'manage';
  return { ...choiceOption(t, `agentStep.${word}`), description: stepWords(t, area, step) };
}

/** The areas of a share, named as the columns of the form are. */
export const AREA_WORDS = { timeline: 'access.agents.area.timeline', chat: 'access.agents.area.chat', documents: 'access.agents.area.documents' } as const;

interface Change {
  area: Area;
  parts: ShareParts;
}

export function AgentRow({ share, target, gives, meId }: { share: AgentShare; target: AccessTarget; gives: readonly AgentLevel[]; meId: string | undefined }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const copy = isCopy(share);
  const reach = reachOf(share);
  const areas = areasOf(target.kind, reach);
  const mine = meId !== undefined && share.by.id === meId;
  const may = mine || mayGiveParts(share.parts, gives);
  const why = may ? undefined : t('access.agents.notYoursToChange');
  const [asking, setAsking] = useState<Change | null>(null);
  const [removing, setRemoving] = useState(false);
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

  const onlyArea = areas.filter((area) => stepOf(area, share.parts) > 0).length <= 1;
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
            disabled={!may}
            onClick={() => setRemoving(true)}
          />
        </div>
      </div>

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
                const needed = levelNeeded(step);
                const off = (needed !== null && !gives.includes(needed)) || (step === 0 && onlyArea);
                return { value: String(step), ...stepOption(t, area, step), disabled: off };
              })}
            />
          ))}
        </div>
      )}

      <p className="text-sm text-text-muted">{t('access.agents.by', { name: share.by.display_name, date })}</p>

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
            <p>
              {t('access.agents.change.line', {
                area: t(AREA_WORDS[asking.area]),
                from: stepWords(t, asking.area, stepOf(asking.area, share.parts)),
                to: stepWords(t, asking.area, stepOf(asking.area, asking.parts)),
              })}
            </p>
            {stepOf(asking.area, asking.parts) === 3 && <p className="text-text-muted">{t('access.agents.manageNote')}</p>}
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
        {copy ? t('access.agents.takeBack.body', { name: share.to.name ?? '' }) : t('access.agents.stop.body')}
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
