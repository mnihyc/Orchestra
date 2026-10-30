/** Passing a limit, from the composer, a row that waits and a blocked thread: Proceed anyway, Pass mine and wait, Change a limit…, and for everybody else who can let them through.
 *
 * What somebody reads when a turn would go past a limit: each limit that stands in its way in the words of
 * its cap, with what is used of how much as a meter, when it starts again, who set it and where, and how
 * often it was passed. Whoever may let work past every one of them reads what they pass and goes on, for
 * that one turn and with their name on it; whoever may pass some and not the rest reads which is which and
 * passes theirs and waits for the others; anybody else reads who could let them through, and may still
 * send, in which case the message waits as it would have. A limit on turns at once says how many run, and
 * a provider's own limit says how long ago its vendor reported it and that the vendor decides.
 */
import { Link } from '@tanstack/react-router';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { CapTitle,CapSetter } from './CapPeople';

import type { LimitBypassRequest, LimitReached, ThreadLimits } from '@/generated/api-types';
import { CapMeter, Limits } from '@/features/usage/Limits';
import { startsAgain, usedWords, whereWords } from '@/features/usage/words';
import { decimal } from '@/features/usage/figures';
import type { ProgramTitle } from '@/features/usage/words';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { formatRelative } from '@/lib/time';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';

export interface LimitPromptProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  limits: ThreadLimits;
  /** Sends the message: past the limits the person may pass (`bypass`), or as it is, to wait. */
  onSend: (bypass: LimitBypassRequest | undefined) => void;
  pending: boolean;
  error?: unknown;
  /** The thread, for opening its limits to change one. */
  threadId?: string | undefined;
}

/** What the person lets past: the caps and the provider's limit they hold the bypass row for. */
export function bypassOf(limits: ThreadLimits): LimitBypassRequest | undefined {
  const caps = limits.reached.flatMap((limit) => (limit.can_bypass && limit.cap !== null && limit.passed_by === null ? [limit.cap.cap.id] : []));
  const provider = limits.reached.some((limit) => limit.can_bypass && limit.provider_limit !== null && limit.passed_by === null);
  if (caps.length === 0 && !provider) return undefined;
  return { ...(caps.length > 0 ? { caps } : {}), ...(provider ? { provider_limit: true } : {}) };
}

/** Whether every limit in the way is one on turns at once, or the provider's own: the words of the buttons and of the line under the title follow. */
export function kindOf(limits: ThreadLimits): 'running' | 'provider' | 'mixed' {
  const kinds = new Set(limits.reached.map((limit) => (limit.cap === null ? 'provider' : limit.cap.cap.meter === 'running' ? 'running' : 'use')));
  if (kinds.size === 1 && kinds.has('running')) return 'running';
  if (kinds.size === 1 && kinds.has('provider')) return 'provider';
  return 'mixed';
}

export function LimitLine({ limit, programTitle }: { limit: LimitReached; programTitle?: ProgramTitle }) {
  const { t } = useTranslation();
  const context = useFormatContext();
  const { locale } = context;
  const status = limit.cap;
  const names = limit.ask.map((person) => person.display_name).join(', ');
  const who = limit.can_bypass ? (
    <p className="text-sm text-text">{t('caps.prompt.you')}</p>
  ) : limit.ask.length > 0 ? (
    <p className="text-sm text-text-muted">{t('caps.prompt.askWho', { names: limit.ask_more > 0 ? t('caps.prompt.askMore', { names, count: limit.ask_more }) : names })}</p>
  ) : (
    <p className="text-sm text-text-muted">{t('caps.prompt.nobody')}</p>
  );
  const passed = limit.passed_by === null ? null : <p className="text-sm text-text">{t('caps.prompt.passedBy', { name: limit.passed_by.display_name })}</p>;

  if (status === null) {
    const provider = limit.provider_limit;
    return (
      <li className="flex flex-col gap-1 rounded-lg border border-border px-3 py-2">
        <p className="text-base font-medium text-text">{t('caps.prompt.providerTitle', { provider: provider?.provider.name ?? '' })}</p>
        {provider !== null && <p className="text-sm text-text-muted">{t('caps.prompt.provider', { provider: provider.provider.name, window: provider.window, when: formatRelative(provider.as_of, context) })}</p>}
        {provider?.resets_at !== null && provider?.resets_at !== undefined && <p className="text-sm text-text-muted">{t('caps.prompt.resets', { when: formatRelative(provider.resets_at, context) })}</p>}
        {passed}
        {who}
      </li>
    );
  }

  const cap = status.cap;
  const running = cap.meter === 'running';
  return (
    <li className="flex flex-col gap-1 rounded-lg border border-border px-3 py-2">
      <p className="text-base font-medium text-text"><CapTitle cap={cap} programTitle={programTitle} /></p>
      {running ? (
        <p className="text-sm text-text">{t('caps.prompt.running', { used: formatNumber(decimal(status.used), locale), amount: formatNumber(decimal(cap.amount), locale), where: whereWords(cap, t) })}</p>
      ) : (
        <>
          <p className="text-sm text-text tabular-nums">{usedWords(status, t, locale)}</p>
          <CapMeter status={status} label={t('caps.row.meter', { percent: Math.round(Math.min(100, (decimal(status.used) / Math.max(1, decimal(cap.amount))) * 100)) })} />
        </>
      )}
      <p className="flex flex-wrap items-center gap-x-3 text-sm text-text-muted">
        {status.period_ends_at !== null && <span>{startsAgain(status.period_ends_at, cap.period, locale, t)}</span>}
        <CapSetter user={cap.created_by} />
        {status.bypasses.count > 0 && <span>{t('caps.row.passed', { count: status.bypasses.count })}</span>}
      </p>
      {passed}
      {who}
    </li>
  );
}

/** Opens the limits of the thread in a card, so that one can be raised or a person exempted: how work goes on past it for longer. */
function ChangeLimit({ threadId, name }: { threadId: string; name?: string }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button variant="ghost" onClick={() => setOpen(true)}>
        {t('caps.prompt.change')}
      </Button>
      <Dialog open={open} onOpenChange={setOpen} title={t('caps.prompt.changeTitle')} description={t('caps.prompt.changeAbout')} size="lg">
        <Limits of={{ kind: 'node', id: threadId, name: name ?? '', manage: false }} bare />
        <p className="py-3 text-sm text-text-muted">
          <Link to="/usage" search={{ scope: `node:${threadId}` }} className="text-accent-text hover:underline" onClick={() => setOpen(false)}>
            {t('caps.prompt.usage')}
          </Link>
        </p>
      </Dialog>
    </>
  );
}

/** The prompt a person meets when a limit stands in the way of the turn they would start. */
export function LimitPrompt({ open, onOpenChange, limits, onSend, pending, error, threadId }: LimitPromptProps) {
  const { t } = useTranslation();
  const bypass = bypassOf(limits);
  const all = limits.bypass_all;
  const kind = kindOf(limits);
  const mayChange = limits.reached.some((limit) => limit.cap?.cap.can_edit === true);
  const proceed = kind === 'running' ? t('caps.prompt.startNow') : kind === 'provider' ? t('caps.prompt.tryAnyway') : all ? t('caps.prompt.proceed') : t('caps.prompt.passMine');
  const wait = kind === 'running' ? t('caps.prompt.wait') : t('caps.prompt.sendAndWait');
  const description = bypass === undefined ? (kind === 'running' ? t('caps.prompt.aboutRunning') : t('caps.prompt.aboutWait')) : all ? t('caps.prompt.aboutYouCan') : t('caps.prompt.aboutSome');
  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={t('caps.prompt.title')}
      description={description}
      dismissible={!pending}
      footer={
        <>
          <Button variant="ghost" disabled={pending} onClick={() => onOpenChange(false)}>
            {t('common.cancel')}
          </Button>
          {mayChange && threadId !== undefined && <ChangeLimit threadId={threadId} />}
          {/* Whoever may pass a limit has the button that sends it past; whoever may pass none may still send, and wait. */}
          {bypass === undefined ? (
            <Button variant="secondary" disabled={pending} onClick={() => onSend(undefined)}>
              {wait}
            </Button>
          ) : (
            <Button variant="primary" pending={pending} onClick={() => onSend(bypass)}>
              {proceed}
            </Button>
          )}
        </>
      }
    >
      <div className="flex flex-col gap-3">
        <ul className="flex flex-col gap-2">
          {limits.reached.map((limit, index) => (
            <LimitLine key={limit.cap?.cap.id ?? `provider-${index}`} limit={limit} />
          ))}
        </ul>
        {bypass !== undefined && !all && <InlineAlert tone="warning">{t('caps.prompt.partial')}</InlineAlert>}
        {error !== undefined && error !== null && <ErrorAlert error={error} />}
      </div>
    </Dialog>
  );
}
