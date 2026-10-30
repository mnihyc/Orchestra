/** The one line that starts a side thread, and what is said when a thread has as many open as it may.
 *
 * Asked from a message of the conversation (which takes the history up to it) or from the menu of the thread: the
 * words, and one button. The side thread is the same agent with the same files, and what is said in it stays in it.
 * At the limit the answer is in place: how many are open, and the open ones, each with a way to close it, so that
 * one can be closed and the question asked again without leaving the dialog.
 */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { sideLimitOf, sidesQuery } from '@/api/forks';
import { nodeQuery } from '@/api/nodes';
import { sideRefusal } from '@/features/side-threads/availability';
import { useAskOnSide, useCloseSide, type OpenedSideAsk, useSideAsk } from '@/features/side-threads/ask';
import { isComposing } from '@/lib/shortcuts';
import { useThreadView } from '@/realtime/stores/thread';
import { isCurrentThreadRead } from '@/api/threadRead';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Textarea } from '@/ui/Textarea';

function AskDialog({ ask, open, instance, onClose }: { ask: OpenedSideAsk; open: boolean; instance: number; onClose: () => void }) {
  const { t } = useTranslation();
  const [text, setText] = useState(ask.text);
  const start = useAskOnSide(ask.read ?? ask.threadId, { toTurn: ask.toTurn, origin: ask.origin, onAccepted: (_reply, question) => {
    if (useSideAsk.getState().instance !== instance) return;
    setText((current) => current.trim() === question.text.trim() ? '' : current);
    onClose();
  } });
  const close = useCloseSide(ask.threadId);
  const view = useThreadView(ask.read ?? ask.threadId);
  const held = view.thread;
  const read = useQuery({ ...nodeQuery(ask.threadId), enabled: open && held === null });
  const thread = held ?? (read.data?.kind === 'thread' ? read.data : null);
  const refused = ask.origin !== undefined && !isCurrentThreadRead(ask.origin) ? t('messageVersions.activeActions') : sideRefusal(thread, t);
  const mode = thread?.sides?.mode;
  const limit = sideLimitOf(start.error);
  const sides = useQuery({ ...sidesQuery(ask.threadId, 'open'), enabled: open && thread !== null && limit !== null });
  // A lost answer may already have made the aside. Retry that exact attempt before editing it.
  const uncertain = start.unconfirmed;
  const shownText = uncertain || start.isPending ? start.variables?.text ?? text : text;
  const empty = shownText.trim() === '';
  const submit = () => {
    if (empty || start.isPending || (refused !== false && !uncertain)) return;
    const question = uncertain && start.variables !== undefined ? start.variables : { text: text.trim(), toTurn: ask.toTurn };
    start.mutate(question);
  };
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !start.isPending && onClose()}
      size="sm"
      title={t('side.ask.title')}
      description={thread === null ? undefined : t('side.ask.about', { thread: thread.name })}
      dismissible={!start.isPending}
      footer={
        <>
          <Button disabled={start.isPending} onClick={onClose}>
            {uncertain ? t('common.close') : t('common.cancel')}
          </Button>
          <Button variant="primary" pending={start.isPending} disabled={empty || (refused !== false && !uncertain)} onClick={submit}>
            {uncertain ? t('common.tryAgain') : t('side.ask.send')}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3 py-2">
        {mode !== null && mode !== undefined && <p className="text-sm text-text-muted">{t('side.ask.mode', { mode: t(`side.reading.${mode}`) })}</p>}
        {refused !== false && <p role="status" className="text-sm text-text-muted">{refused}</p>}
        {read.error !== null && held === null && <ErrorAlert error={read.error} />}
        <Textarea
          label={t('side.ask.label')}
          hideLabel
          rows={3}
          className="resize-none"
          aria-keyshortcuts="Enter Control+Enter Meta+Enter"
          value={shownText}
          disabled={start.isPending || uncertain}
          onChange={(event) => setText(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.altKey && !event.shiftKey && !isComposing(event.nativeEvent)) {
              event.preventDefault();
              event.stopPropagation();
              submit();
            }
          }}
        />
        {uncertain && <p role="status" className="text-sm text-text-muted">{t('side.ask.unconfirmed')}</p>}
        {ask.toTurn !== null && <p className="text-sm text-text-muted">{t('side.ask.fromTurn', { turn: ask.toTurn })}</p>}
        {limit !== null && thread !== null ? (
          <InlineAlert tone="warning" title={t('side.limit.title', { name: thread.name, count: limit.open })}>
            <span className="flex flex-col gap-2">
              <span>{t('side.limit.about')}</span>
              <ul className="flex flex-col gap-1" aria-label={t('side.limit.open')}>
                {(sides.data ?? []).map((side) => (
                  <li key={side.id} className="flex items-center justify-between gap-2">
                    <span className="min-w-0 truncate text-text">{side.name}</span>
                    <Button size="sm" pending={close.isPending && close.variables === side.id} onClick={() => close.mutate(side.id, { onSuccess: () => start.reset() })}>
                      {t('side.close')}
                    </Button>
                  </li>
                ))}
              </ul>
            </span>
          </InlineAlert>
        ) : (
          !uncertain && start.error !== null && <ErrorAlert error={start.error} />
        )}
      </div>
    </Dialog>
  );
}

/** Draws the dialog that was asked for with `openSideAsk`; stands once in the frame, beside the wizard. */
export function SideAskHost() {
  const current = useSideAsk((state) => state.current);
  const open = useSideAsk((state) => state.open);
  const hide = useSideAsk((state) => state.hide);
  const instance = useSideAsk((state) => state.instance);
  if (current === null) return null;
  return <AskDialog key={instance} ask={current} open={open} instance={instance} onClose={hide} />;
}
