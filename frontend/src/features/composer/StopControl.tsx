/** Explicit and keyboard interruption, for whoever may drive the thread.
 *
 * Stopping the turn that runs, or the start of the next one while there is no turn yet, for whoever may talk to the agent:
 * a button, and the key. With an empty box and a turn running, the first press of `Esc` says "Press Esc again to stop", and a
 * second within two seconds stops it. The server answers at once and says that the thread is stopping; the button says
 * "Stopping…", with the time that has gone by once it lasts more than a second, until the program has stopped.
 */
import { useCompactControls } from '@/features/workbench/Narrow';
import { useMutation } from '@tanstack/react-query';
import { Square } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { interruptTurn } from '@/api/threads';
import { useFormatContext } from '@/lib/appearance';
import { formatSpan, useRunningClock } from '@/lib/time';
import { runningTurn, threadWriter, useThreadPart } from '@/realtime/stores/thread';
import { stoppingSince } from '@/lib/threadStopping';
import { threadReceipt } from '@/realtime/threadScope';
import { useStopRequest } from './stopState';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import type { MenuEntry } from '@/ui/Menu';
import { SplitButton } from '@/ui/SplitButton';

/** The second press of Esc has to come within this long after the first. */
export const ESC_AGAIN_WITHIN_MS = 2_000;

/** The stop of a turn, or of a start that has no turn yet (`starting`), and the Esc that does it. */
export function useStop(threadId: string, turnId: string | null, live: boolean, starting = false) {
  const read = useThreadReadRef(threadId);
  const [armed, setArmed] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const request = useStopRequest(threadId);
  const storedSince = useThreadPart(threadId, (view) => stoppingSince(view.thread, runningTurn(view)));
  const stop = useMutation({
    mutationKey: request.key,
    mutationFn: () => interruptTurn(read, turnId),
    onSuccess: (answer) => {
      if (answer.stopping !== null && answer.stopping !== undefined) threadWriter.stopping(read, answer.stopping, threadReceipt(answer));
      if (answer.turn !== undefined && answer.turn !== null) threadWriter.apply(read, { turns: [answer.turn] });
    },
  });
  const may = turnId !== null || starting || storedSince !== null;
  const since = storedSince;
  const pending = request.pending;
  const reset = stop.reset;
  useEffect(() => { if (!may) reset(); }, [may, reset]);
  useEffect(() => () => clearTimeout(timer.current), []);
  /** Called for an Esc in an empty box: the first press arms, the second stops. */
  const escape = useCallback(() => {
    if (!may || !live || pending || since !== null) return;
    if (armed) {
      clearTimeout(timer.current);
      setArmed(false);
      stop.mutate();
      return;
    }
    setArmed(true);
    timer.current = setTimeout(() => setArmed(false), ESC_AGAIN_WITHIN_MS);
  }, [armed, may, live, pending, since, stop]);
  return { stop, armed, escape, since, may, pending };
}

/** What the Stop button says: its own word, and while the stop is under way "Stopping…" with the time once it lasts more than a second. */
function useStopLabel(stop: ReturnType<typeof useStop>): { label: string; stopping: boolean } {
  const { t } = useTranslation();
  const format = useFormatContext();
  const stopping = stop.pending || stop.since !== null;
  const now = useRunningClock(stopping);
  if (!stopping) return { label: t('composer.stop'), stopping };
  const since = stop.since === null ? Number.NaN : Date.parse(stop.since);
  const lasted = Number.isFinite(since) ? now - since : 0;
  return { label: lasted >= 1_000 ? t('composer.stoppingTime', { time: formatSpan(lasted, format) }) : t('composer.stopping'), stopping };
}

export function StopControl({ stop, entries, disabled = false, menuDisabled, reason, inlineFeedback = true }: {
  stop: ReturnType<typeof useStop>;
  entries?: readonly MenuEntry[];
  disabled?: boolean;
  /** Delivery choices can be ready before the current turn can be stopped. */
  menuDisabled?: boolean;
  reason?: string | undefined;
  inlineFeedback?: boolean;
}) {
  const compact = useCompactControls();
  const { t } = useTranslation();
  const { label, stopping } = useStopLabel(stop);
  return (
    <>
      {entries === undefined ? <Button size="sm" data-tutorial-target="thread-stop" variant="secondary" icon={<Square aria-hidden />} pending={stopping} disabled={disabled} onClick={() => stop.stop.mutate()}>
        {label}
      </Button> : <SplitButton size="sm" variant="secondary" className={compact ? '[&_button]:gap-1 [&_button]:px-1 [&_button]:text-xs [&_button[data-action=stop]>svg:not(.animate-spin)]:hidden' : undefined} icon={<Square aria-hidden />} pending={stopping} disabled={disabled}
        menuDisabled={(menuDisabled ?? disabled) || stop.pending} tutorialTarget="thread-stop" menuLabel={t('composer.deliveryMenu')} entries={entries} onClick={() => stop.stop.mutate()} {...(reason === undefined ? {} : { reason })}>
        {label}
      </SplitButton>}
      {inlineFeedback && <StopFeedback stop={stop} />}
    </>
  );
}

/** Stop confirmation and failures stand below the row, where they cannot push Send off a phone. */
export function StopFeedback({ stop }: { stop: ReturnType<typeof useStop> }) {
  const { t } = useTranslation();
  return <>
    {stop.armed && (
        <span role="status" className="text-sm text-text-muted">
          {t('composer.escAgain')}
        </span>
      )}
      {stop.stop.error !== null && <ErrorAlert error={stop.stop.error} className="w-full" />}
  </>;
}
