/** Explicit and keyboard interruption, for whoever may drive the thread.
 *
 * Stopping the turn that runs, for whoever may talk to the agent: a button, and the key. With an empty box
 * and a turn running, the first press of `Esc` says "Press Esc again to stop", and a second within two
 * seconds stops it. The stop is shown as under way until the server has answered; nothing is assumed.
 */
import { useCompactControls } from '@/features/workbench/Narrow';
import { useMutation } from '@tanstack/react-query';
import { Square } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { interruptTurn } from '@/api/threads';
import { threadWriter } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import type { MenuEntry } from '@/ui/Menu';
import { SplitButton } from '@/ui/SplitButton';

/** The second press of Esc has to come within this long after the first. */
export const ESC_AGAIN_WITHIN_MS = 2_000;

/** The stop of a turn, and the Esc that does it. */
export function useStop(threadId: string, turnId: string | null, live: boolean) {
  const read = useThreadReadRef(threadId);
  const [armed, setArmed] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const stop = useMutation({
    mutationFn: () => interruptTurn(read, turnId ?? ''),
    onSuccess: (turn) => threadWriter.apply(read, { turns: [turn] }),
  });
  useEffect(() => () => clearTimeout(timer.current), []);
  /** Called for an Esc in an empty box: the first press arms, the second stops. */
  const escape = useCallback(() => {
    if (turnId === null || !live || stop.isPending) return;
    if (armed) {
      clearTimeout(timer.current);
      setArmed(false);
      stop.mutate();
      return;
    }
    setArmed(true);
    timer.current = setTimeout(() => setArmed(false), ESC_AGAIN_WITHIN_MS);
  }, [armed, turnId, live, stop]);
  return { stop, armed, escape };
}

export function StopControl({ stop, entries, disabled = false, reason, inlineFeedback = true }: {
  stop: ReturnType<typeof useStop>;
  entries?: readonly MenuEntry[];
  disabled?: boolean;
  reason?: string | undefined;
  inlineFeedback?: boolean;
}) {
  const compact = useCompactControls();
  const { t } = useTranslation();
  return (
    <>
      {entries === undefined ? <Button size="sm" data-tutorial-target="thread-stop" variant="secondary" icon={<Square aria-hidden />} pending={stop.stop.isPending} disabled={disabled} onClick={() => stop.stop.mutate()}>
        {t('composer.stop')}
      </Button> : <SplitButton size="sm" variant="secondary" className={compact ? '[&_button]:gap-1 [&_button]:px-1 [&_button]:text-xs [&_button[data-action=stop]>svg:not(.animate-spin)]:hidden' : undefined} icon={<Square aria-hidden />} pending={stop.stop.isPending} disabled={disabled}
        menuDisabled={disabled || stop.stop.isPending} tutorialTarget="thread-stop" menuLabel={t('composer.deliveryMenu')} entries={entries} onClick={() => stop.stop.mutate()} {...(reason === undefined ? {} : { reason })}>
        {t('composer.stop')}
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
