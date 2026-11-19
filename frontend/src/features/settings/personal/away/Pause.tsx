/** One confirmed pause control and explicit time form, shared by the editor, Inbox and scoped bell. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Check, Clock, Pause, Play, RefreshCw } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { awayPrefsQuery, changePrefs } from '@/api/away';
import { ApiError } from '@/api/client';
import { useAppearance } from '@/lib/appearance';
import { useNow } from '@/lib/time';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { DateField } from '@/ui/DateField';
import { suppliedPrefs, type AwayPrefs } from './types';
import { activePause, futureInZone, writePause } from './time';

/** Browser timeouts use a signed 32-bit delay; the existing shared clock revisits longer pauses. */
const MAX_BROWSER_TIMER_MS = 2_147_483_647;

export function PauseControls() {
  const { t } = useTranslation();
  const query = useQuery(awayPrefsQuery);
  const [mode, setMode] = useState<'pause' | 'resume' | null>(null);
  const now = useNow();
  const prefs = query.data;
  const [endReached, setEndReached] = useState<string | null>(null);
  const pauseEnd = prefs?.pause_until ?? null;
  useEffect(() => {
    if (pauseEnd === null || endReached === pauseEnd) return;
    const remaining = Date.parse(pauseEnd) - Date.now();
    if (!Number.isFinite(remaining) || remaining > MAX_BROWSER_TIMER_MS) return;
    // A single local deadline hides the status at expiry, without an HTTP read or waiting for relative-time rounding.
    const timer = setTimeout(() => setEndReached(pauseEnd), Math.max(0, remaining));
    return () => clearTimeout(timer);
  }, [pauseEnd, now, endReached]);
  const zone = useAppearance((state) => state.timeZone);
  const locale = useAppearance((state) => state.locale);
  if (prefs === undefined) return query.isPending ? <p role="status" className="text-sm text-text-muted">{t('away.checking')}</p> : <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  // Old servers do not carry a revision. A form without one cannot safely save or clear a pause.
  if (suppliedPrefs(prefs) === null) return null;
  const until = endReached === prefs.pause_until ? null : activePause(prefs.pause_until, now);
  const paused = until !== null;
  return (
    <div className="inline-flex min-w-0 flex-wrap items-center gap-2" data-away-pause>
      <div hidden={mode !== null} className={mode === null ? 'flex flex-wrap items-center gap-2' : 'hidden'}>
        {until !== null && mode === null && <p className="flex items-center gap-2 text-sm text-text wrap-anywhere" role="status"><Clock aria-hidden className="size-4 shrink-0" />{t('away.pause.until', { time: writePause(until, zone, locale) })}</p>}
        <Button size="sm" icon={paused ? <Play aria-hidden /> : <Pause aria-hidden />} onClick={() => setMode(paused ? 'resume' : 'pause')}>{t(paused ? 'away.pause.resume' : 'away.pause.button')}</Button>
        {paused && <Button size="sm" variant="ghost" icon={<Clock aria-hidden />} onClick={() => setMode('pause')}>{t('away.pause.change')}</Button>}
      </div>
      {mode !== null && <PauseDialog open onOpenChange={(open) => { if (!open) setMode(null); }} initial={prefs} mode={mode} />}
    </div>
  );
}

export function PauseDialog({ open, onOpenChange, initial, mode }: { open: boolean; onOpenChange: (open: boolean) => void; initial: AwayPrefs; mode?: 'pause' | 'resume' }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const zone = useAppearance((state) => state.timeZone);
  const locale = useAppearance((state) => state.locale);
  const [date, setDate] = useState('');
  const [time, setTime] = useState('');
  const [revision, setRevision] = useState(initial.revision);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [fault, setFault] = useState<string | null>(null);
  const [stale, setStale] = useState<AwayPrefs | null>(null);
  const now = useNow();
  const until = activePause((stale ?? initial).pause_until, now);
  const [resume, setResume] = useState(mode === 'resume' || (mode === undefined && initial.pause_until !== null));
  const submit = async () => {
    if (saving) return;
    const choice = resume ? { instant: null } : futureInZone(date, time, zone, Date.now());
    if ('error' in choice) { setFault(t(`away.pause.errors.${choice.error}`)); return; }
    setSaving(true); setError(null); setFault(null);
    try {
      await changePrefs({ expected_revision: revision, pause_until: choice.instant });
      onOpenChange(false);
    } catch (failure) {
      setError(failure);
      if (failure instanceof ApiError && failure.is('conflict.stale_revision')) {
        const current = await client.query({ ...awayPrefsQuery, staleTime: 0 }).catch(() => null);
        setStale(current);
      }
    } finally { setSaving(false); }
  };
  return (
    <Dialog open={open} onOpenChange={onOpenChange} title={t('away.pause.title')} description={t('away.pause.explanation')} dismissible={!saving}
      footer={<><Button onClick={() => onOpenChange(false)} disabled={saving}>{t('common.cancel')}</Button><Button variant="primary" icon={error === null ? (resume ? <Play aria-hidden /> : <Pause aria-hidden />) : <RefreshCw aria-hidden />} pending={saving} disabled={stale !== null} onClick={() => void submit()}>{t(error === null ? (resume ? 'away.pause.resume' : 'away.pause.confirm') : 'away.retry')}</Button></>}>
      <form className="flex flex-col gap-3 pb-2" onSubmit={(event) => { event.preventDefault(); if (stale === null) void submit(); }}>
        <p role="status" className="text-sm text-text-muted">{until === null ? t('away.pause.unpaused') : t('away.pause.until', { time: writePause(until, zone, locale) })}</p>
        <p className="text-sm font-medium text-text">{t('away.pause.zone', { zone })}</p>
        {resume ? <Button size="sm" variant="ghost" icon={<Clock aria-hidden />} onClick={() => setResume(false)}>{t('away.pause.change')}</Button> : <div className="grid grid-cols-1 gap-3 sm:grid-cols-2"><DateField label={t('away.pause.date')} value={date} onValueChange={setDate} disabled={saving} /><Input label={t('away.pause.time')} type="time" value={time} onChange={(event) => setTime(event.target.value)} disabled={saving} /></div>}
        {fault !== null && <InlineAlert tone="danger">{fault}</InlineAlert>}
        {stale !== null ? <InlineAlert tone="warning" title={t('away.pause.stale')}><p>{t('away.pause.proposalKept')}</p><Button size="sm" icon={<Check aria-hidden />} onClick={() => { setRevision(stale.revision); setStale(null); setError(null); }}>{t('away.pause.useCurrent')}</Button></InlineAlert> : error !== null && <ErrorAlert error={error} />}
        <p className="text-sm text-text-muted">{t('away.pause.boundary')}</p>
      </form>
    </Dialog>
  );
}
