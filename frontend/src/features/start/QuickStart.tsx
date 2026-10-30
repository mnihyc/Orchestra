/** Create topic/thread/input from scope defaults, with model, effort and environment changeable in the line below.
 *
 * The box that heads a topic ("Start a thread…") and a project ("Start a topic…"). Where the defaults of the place settle what
 * a thread there runs on, a person types what they want and presses Enter: the topic around the thread (from a project),
 * the environment where it is to be a new one, and the thread are made with what they typed as its first message, and the
 * thread opens. One line under the box says what it will run on, and Change opens the dialog with all of it filled in.
 * Where something is missing the same place says which, asks for that one choice and, for whoever may, offers to keep it as
 * the default; typing is never held back, only the sending, and nothing is made until it can be sent. What was typed
 * stays where a step failed, with what was made so far said.
 */
import { Send } from 'lucide-react';
import { type KeyboardEvent, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { placeOfNode } from '@/features/navigation/places';
import type { Project, Topic } from '@/generated/api-types';
import { canCreateIn } from '@/lib/permissions';
import { isComposing } from '@/lib/shortcuts';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Skeleton } from '@/ui/Skeleton';
import { Textarea } from '@/ui/Textarea';

import { NewThreadDialog } from './BindingDialog';
import { type Mode, NO_PICKS, type Picks, settingsToKeep } from './choices';
import { MissingDefault, useScopes, widestScope } from './MissingDefault';
import { useStartOptions } from './options';
import { useStart } from './start';
import { bindingReason, runsLine } from './words';

/** Defaults, the last successful setup, then the first usable setup; the line always names what Enter will start. */
const QUICK_SETUP: Mode = { suggest: true, respectDefaults: true };

export function canQuickStart(node: Topic | Project): boolean {
  return canCreateIn(node.kind, node.access) && !node.archived;
}

export function QuickStart({ node }: { node: Topic | Project }) {
  // Whoever may not start work here reads what the screen already says; the starter is for the one who may.
  return canQuickStart(node) ? <Composer node={node} /> : null;
}

function Composer({ node }: { node: Topic | Project }) {
  const { t } = useTranslation();
  const place = placeOfNode(node);
  const [text, setText] = useState('');
  const [picks, setPicks] = useState<Picks>(NO_PICKS);
  const [keepChosen, setKeepChosen] = useState<string | null | undefined>(undefined);
  const [changing, setChanging] = useState(false);
  const { options, resolved, loading, failed, refreshing, retry } = useStartOptions(node.id, picks, QUICK_SETUP);
  const scopes = useScopes(node);
  const keepAt = keepChosen === undefined ? (widestScope(scopes)?.id ?? null) : keepChosen;
  const make = useStart(() => {
    setText('');
    setPicks(NO_PICKS);
    setKeepChosen(undefined);
  });

  const message = text.trim();
  // A choice that was made here for the thread stays in view with the way to keep it, until it has been kept or the thread has started.
  const kept = options !== null && resolved !== null && resolved.missing.length === 0 && (picks.provider !== null || picks.agent !== null || picks.model !== null) && settingsToKeep(options, resolved).length > 0;
  const ready = options !== null && resolved !== null && resolved.missing.length === 0 && !refreshing && !resolved.settling;
  const send = () => {
    if (!ready || message === '' || make.pending) return;
    make.start({
      place,
      options,
      resolved,
      name: '',
      directory: '',
      privately: false,
      message,
    });
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    // A line is started with Shift+Enter; Enter alone sends, and a composition in progress is left alone.
    if (event.key !== 'Enter' || event.shiftKey || isComposing(event.nativeEvent)) return;
    event.preventDefault();
    send();
  };

  const why = ready ? null : loading || refreshing ? t('start.quick.checking') : resolved?.missing[0] !== undefined ? t('start.quick.missingWhy') : null;
  const kindKey = node.kind === 'topic' ? 'topic' : 'project';
  return (
    <section aria-label={t(`start.quick.label.${kindKey}`)} className="flex flex-col gap-2" data-quick-start>
      <form
        className="flex items-end gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          send();
        }}
      >
        <Textarea
          label={t(`start.quick.label.${kindKey}`)}
          hideLabel
          placeholder={t(`start.quick.placeholder.${kindKey}`)}
          rows={1}
          value={text}
          onChange={(event) => setText(event.target.value)}
          onKeyDown={onKeyDown}
          disabled={make.pending}
          fieldClassName="min-w-0 flex-1"
        />
        <Button action="send" type="submit" variant="primary" icon={<Send aria-hidden />} pending={make.pending} blocked={!ready || message === ''} title={why ?? undefined}>
          {t(node.kind === 'project' ? 'screenReview.startTopic' : 'start.quick.send')}
        </Button>
      </form>

      {failed !== null ? (
        <ErrorState compact error={failed} title={t('start.quick.failed')} onRetry={retry} />
      ) : options === null || resolved === null ? (
        // The line it becomes is as high as this, so that what stands under the box does not move when the answer arrives.
        <div className="flex min-h-(--control-sm) items-center">
          <Skeleton className="h-4 w-2/3" />
        </div>
      ) : resolved.missing.length > 0 ? (
        <MissingDefault place={place} node={node} options={options} resolved={resolved} picks={picks} onPick={(patch) => setPicks((before) => ({ ...before, ...patch }))} scopes={scopes} keepAt={keepAt} onKeepAt={setKeepChosen} onChange={() => setChanging(true)} />
      ) : (
        <>
          <p className="flex min-h-(--control-sm) flex-wrap items-center gap-x-2 text-sm text-text-muted" data-runs-on>
            <span aria-live="polite">{make.pending && make.step !== null ? t(`start.making.${make.step}`) : runsLine(resolved, t)}</span>
            <Button size="sm" variant="ghost" disabled={make.pending} onClick={() => setChanging(true)}>
              {t('start.quick.change')}
            </Button>
          </p>
          {kept && (
            <MissingDefault place={place} node={node} options={options} resolved={resolved} picks={picks} onPick={(patch) => setPicks((before) => ({ ...before, ...patch }))} scopes={scopes} keepAt={keepAt} onKeepAt={setKeepChosen} onChange={() => setChanging(true)} />
          )}
        </>
      )}

      {make.error !== null && (
        <div className="flex flex-col gap-2">
          <ErrorAlert error={make.error} message={bindingReason(make.error, t) ?? undefined} action={<Button action="retry" size="sm" disabled={!ready} onClick={send}>{t('startRecovery.retry')}</Button>} />
          <Button size="sm" className="self-start" onClick={() => setChanging(true)}>{t('startRecovery.elsewhere')}</Button>
          {(make.standing.topic !== null || make.standing.environment !== null) && (
            <InlineAlert tone="info">
              {[make.standing.topic === null ? null : t('start.made.topic', { name: make.standing.topic.name }), make.standing.environment === null ? null : t('start.made.environment', { name: make.standing.environment.name })]
                .filter((line): line is string => line !== null)
                .join(' ')}
            </InlineAlert>
          )}
        </div>
      )}
      {changing && <NewThreadDialog parent={place} open onClose={() => setChanging(false)} draft={{ picks, message: text }} />}
    </section>
  );
}
