/** IME-aware input to the agent and delivery controls; references added from files, differences and shells; and the limit prompt before a message goes past a limit.
 *
 * Where a person talks to the agent. Everything typed here goes to the agent, never to the people of the
 * thread. The box grows from one line to twelve and then scrolls; a key is on screen at once and the text
 * is kept as a draft. Enter queues, Shift+Enter starts a line, Mod+Enter
 * steers the turn that runs and Mod+Shift+Enter stops it and goes first; with the box empty and a turn
 * running, Esc twice stops it, and Up takes your last queued message back to edit. The split action is
 * Send while idle and Stop while a turn runs, with queue, steer and interruption in its menu. The row
 * also holds references, binding, model and effort, native approval mode and context. Before a message
 * goes past a limit, the limit is read first.
 */
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { useViewInstance } from '@/features/workbench/instance';
import { useQuery } from '@tanstack/react-query';
import { type KeyboardEvent, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { metaQuery } from '@/api/auth';
import { threadContextQuery } from '@/api/context';
import { settingsQuery } from '@/api/settings';
import { limitsQuery, parametersQuery } from '@/api/threads';
import type { Delivery, Thread } from '@/generated/api-types';
import { AttachButton, AttachmentChips, carriesFiles, UploadChips } from '@/features/composer/Attachments';
import { DropTarget } from '@/features/composer/DropTarget';
import { droppedReference, insertAtSelection } from '@/lib/drop';
import { useFileDrops } from '@/features/composer/FileDrops';
import { useViewShown } from '@/features/workbench/viewOpening';
import { type DeliveryState, whyNot } from '@/features/composer/DeliveryMenu';
import { DeliveryControl } from '@/features/composer/DeliveryControl';
import { DraftMark } from '@/features/composer/DraftMark';
import { ComposerControls, ReadOnlyComposerControls } from '@/features/composer/Controls';
import { ReferencePicker, type ReferenceKind } from '@/features/composer/Mentions';
import { addToMessage } from '@/features/files/AddToMessage';
import { useAskOnSide } from '@/features/side-threads/ask';
import { sideReadingAvailable, sideRefusal } from '@/features/side-threads/availability';
import { composerWriter, submitInput, useAttachmentParts, useBox, useComposerStore, useDraftSync } from '@/features/composer/Draft';
import { ModelEffort } from '@/features/composer/ModelEffort';
import { PlanSwitch } from '@/features/composer/PlanSwitch';
import type { Kind } from '@/features/composer/modelState';
import { contextStart, statesOf } from '@/features/composer/modelState';
import { useModelCommands, useSlashMenu } from '@/features/composer/SlashMenu';
import { useGoalCommand } from '@/features/goal/slash';
import { StopFeedback, useStop } from '@/features/composer/StopControl';
import { isApplePlatform, isComposing } from '@/lib/shortcuts';
import { holds } from '@/lib/permissions';
import { attachToThread, releaseUpload } from '@/lib/uploads';
import { pasteContext, pasteKey, pastedPartFits, pastedPartTokens, DEFAULT_PASTE_INLINE_CONTEXT_SHARE } from '@/lib/pastedText';
import { estimateTokenCounts, modelTokenEstimate } from '@/lib/tokenEstimate';
import { usePastedText } from '@/features/composer/usePastedText';
import { useUploads } from '@/lib/useUploads';
import { useSocketStatus } from '@/realtime/live';
import { runningTurn, turnsRevision, useThreadPart } from '@/realtime/stores/thread';
import { cx } from '@/ui/cx';
import { ErrorAlert } from '@/ui/ErrorState';
import { LimitPrompt } from '@/ui/LimitPrompt';
import { Textarea } from '@/ui/Textarea';
import { Button } from '@/ui/Button';
import { programNameIn } from '@/features/resources/programNames';

/** At this share of the input limit a counter appears. */
const COUNTER_FROM = 0.9;
/** The box is never taller than this many lines of text. */
const MOST_LINES = 12;

export function Composer({ thread, onSent }: { thread: Thread; onSent?: () => void }) {
  const { t, i18n } = useTranslation();
  const threadId = thread.id;
  const shown = useViewShown();
  const read = useThreadReadRef(threadId);
  const instance = useViewInstance();
  const socket = useSocketStatus();
  const live = socket.phase === 'ready';
  const { text, setText, refusal } = useBox(threadId);
  const parts = useAttachmentParts(threadId);
  const fileInput = useRef<HTMLInputElement>(null);
  const uploads = useUploads(threadId, (blob, signal) => attachToThread(threadId, blob, signal), (part) => composerWriter.addPart(threadId, part));
  const fileDrops = useFileDrops(`thread:${threadId}`);
  const box = useRef<HTMLTextAreaElement>(null);
  const removePart = (index: number) => {
    const part = parts[index];
    if (part !== undefined && 'blob' in part && part.blob !== null) void releaseUpload(part.blob, { thread: threadId, path: part.type === 'image' ? `.orch/attachments/${part.name}` : 'path' in part ? part.path : '' }).catch(() => undefined);
    if (part?.type === 'pasted_text') pastes.cancel(part);
    composerWriter.setParts(threadId, parts.filter((_, at) => at !== index));
  };
  const pastes = usePastedText({ kind: 'thread', id: threadId, box, maxLines: MOST_LINES, text, setText, remove: removePart,
    add: (part) => composerWriter.addPart(threadId, part),
    replace: (original, part) => composerWriter.setParts(threadId, (useComposerStore.getState().parts[threadId] ?? []).map((held) => held.type === 'pasted_text' && pasteKey(held) === pasteKey(original) ? part : held)),
  });
  const composing = useRef(false);
  const [prompt, setPrompt] = useState<Delivery | null>(null);
  const [reference, setReference] = useState<{ kind: ReferenceKind; query: string } | null>(null);
  // The model control, opened by a command of the box on the choice it names.
  const [control, setControl] = useState<{ open: boolean; section: Kind | null }>({ open: false, section: null });

  const turn = useThreadPart(threadId, runningTurn);
  const active = turn !== null || thread.state === 'running' || thread.state === 'waiting_approval';
  const starting = thread.state === 'starting';
  const mayDrive = holds(thread.access, 'thread.interact');
  const parametersState = useQuery({ ...parametersQuery(threadId), enabled: mayDrive && shown });
  const parameters = parametersState.data;
  const model = thread.model ?? parameters?.model.value;
  const program = thread.binding?.program;
  const buildOff = program?.build?.check?.off ?? [];
  const featureReason = (feature: string) => program === undefined || program === null ? undefined : t('programVersions.featureOff', { feature: t(`programVersions.features.${feature}`, { defaultValue: t('programVersions.disabledFeatures.other') }), name: programNameIn(thread.binding?.agent, i18n.resolvedLanguage ?? i18n.language) ?? t('resources.harnessNameUnknown'), version: program.version });
  const imagesUnsupported = parts.some((part) => 'type' in part && part.type === 'image') && (buildOff.includes('input_images') || parameters?.model.options.find((option) => option.model_name === model)?.parameters.input_images === false);
  const agent = thread.binding?.agent.name ?? '';
  const agentId = thread.binding?.agent.id ?? '';
  const noSteer = useComposerStore((held) => held.cannotSteer[agentId] === true) || buildOff.includes('steer');
  const meta = useQuery({ ...metaQuery, enabled: shown }).data;
  const maxChars = meta?.limits.max_input_chars ?? null;

  const settings = useQuery({ ...settingsQuery(threadId), enabled: mayDrive && shown });
  const configured = settings.data?.find((setting) => setting.key === 'collab.default_delivery')?.value;
  const pasteShare = settings.data?.find((setting) => setting.key === 'composer.paste_inline_context_share')?.value;
  const inlineShare = typeof pasteShare === 'number' ? pasteShare : DEFAULT_PASTE_INLINE_CONTEXT_SHARE;
  const pasteOption = parameters?.model.options.find((option) => option.model_name === model);
  const pasteModel = pasteOption?.parameters;
  const pasteEstimate = modelTokenEstimate(pasteOption);
  const pasteEstimateKnown = estimateTokenCounts(0, 0, pasteEstimate) !== null;
  const pasteWindow = parameters === undefined || pasteModel === undefined ? undefined : statesOf(parameters, null).find((state) => state.kind === 'context')?.force ?? contextStart(pasteModel) ?? pasteModel.context_window;
  const contextRevision = useThreadPart(threadId, turnsRevision);
  const hasPastedText = parts.some((part) => part.type === 'pasted_text');
  const needsPasteFacts = parts.some((part) => part.type === 'pasted_text' && (part.delivery === undefined || part.delivery === null));
  const contextState = useQuery({ ...threadContextQuery(threadId, 'kept', contextRevision), enabled: mayDrive && shown && hasPastedText });
  const context = contextState.data;
  const pasteFactsReady = settings.isSuccess && (inlineShare === 0 || inlineShare === 100 || parametersState.isSuccess && contextState.isSuccess);
  const pasteFactsError = hasPastedText ? parametersState.error ?? settings.error ?? contextState.error : null;
  const attachmentsWaiting = uploads.blocked || pastes.blocked || fileDrops.blocked || needsPasteFacts && !pasteFactsReady || buildOff.includes('input_images') && imagesUnsupported;
  const pasteUsage = useMemo(() => pasteContext(thread.usage, context?.fill), [thread.usage, context?.fill]);
  const pasteDelivery = useMemo(() => {
    const modes = new Map<object, boolean>();
    let used = 0;
    for (const part of parts) if (part.type === 'pasted_text') {
      if (!pasteFactsReady && (part.delivery === undefined || part.delivery === null)) continue;
      const inline = pastedPartFits(part, pasteUsage, pasteEstimate, inlineShare, pasteWindow, used);
      modes.set(part, inline);
      const tokens = pastedPartTokens(part, pasteEstimate);
      if (inline && tokens !== null) used += tokens;
    }
    return modes;
  }, [parts, pasteUsage, pasteEstimate, inlineShare, pasteWindow, pasteFactsReady]);
  const preferred: Delivery = configured === 'steer' || configured === 'interrupt' ? configured : 'queue';
  const state: DeliveryState = { active, starting, live, cannotSteer: noSteer, agent, ...(buildOff.includes('steer') ? { steerUnavailable: featureReason('steer') } : {}) };
  // The usual way, unless it cannot be used now.
  const main: Delivery = active && whyNot(preferred, state, t) === false ? preferred : 'queue';

  // The Plan switch: where the person set it, or on for a thread's first message where the place plans first.
  const planOffered = thread.binding?.plan_mode === true;
  const planUnavailable = program?.build?.check?.off.includes('plan_mode') === true
    ? featureReason('plan_mode') : undefined;
  const planFirst = settings.data?.find((setting) => setting.key === 'thread.plan_first')?.value === true;
  const chosenPlan = useComposerStore((held) => held.planning[threadId]);
  const planning = planOffered && (chosenPlan ?? (planFirst && thread.usage === null && thread.current_turn_id === null));

  const limits = useQuery({ ...limitsQuery(threadId), enabled: mayDrive && shown });
  const stop = useStop(threadId, turn?.id ?? null, live);
  const draft = useDraftSync(threadId, mayDrive, main, shown);

  // Text and cell-width changes use the same existing twelve-line bound.
  useEffect(() => {
    const element = box.current;
    if (element === null) return;
    const resize = () => {
      element.style.height = 'auto';
      const style = getComputedStyle(element);
      const lineHeight = Number.parseFloat(style.lineHeight) || 20;
      const padding = (Number.parseFloat(style.paddingTop) || 0) + (Number.parseFloat(style.paddingBottom) || 0);
      const border = element.offsetHeight - element.clientHeight;
      element.style.height = `${Math.min(element.scrollHeight + border, lineHeight * MOST_LINES + padding + border)}px`;
    };
    resize();
    let width = element.clientWidth;
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => {
      if (element.clientWidth === width) return;
      width = element.clientWidth; resize();
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [text]);

  const fileReason = thread.environment === null ? t('composer.plus.noEnvironment') : !thread.environment_permissions.includes('files.view') ? t('composer.plus.noFileAccess') : false;
  const commands = [
    ...useModelCommands(thread, (section) => setControl({ open: true, section })),
    ...useGoalCommand(thread),
    ...(['file', 'document'] as const).map((kind) => ({ name: kind, about: t(kind === 'file' ? 'composer.plus.file' : 'composer.plus.document'), off: !mayDrive ? t('composer.row.readOnly') : kind === 'file' && fileReason !== false ? fileReason : null,
      run: (query: string) => { setReference({ kind, query }); return null; } })),
  ];
  const slash = useSlashMenu(text, commands, () => setText(''));

  const tooLong = maxChars !== null && text.length > maxChars;
  const empty = text.trim() === '' && parts.length === 0;
  const reached = limits.data !== undefined && limits.data.reached.length > 0;

  const send = (delivery: Delivery) => {
    if (empty || tooLong || !mayDrive || attachmentsWaiting) return;
    if (whyNot(delivery, state, t) !== false) return;
    // A person who already said that the next message may go past what is in its way is not asked again.
    const letPass = useComposerStore.getState().passNext[threadId];
    if (reached && letPass === undefined) {
      setPrompt(delivery);
      return;
    }
    const words = text;
    const content = [...(words === '' ? [] : [{ type: 'text' as const, text: words }]), ...parts];
    setText('');
    composerWriter.setParts(threadId, []);
    composerWriter.letNextPass(threadId, undefined);
    submitInput(read, content, delivery, letPass, undefined, planning);
    onSent?.();
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    // A command of the box takes its keys first, unless a composition is under way: Enter then confirms what is composed.
    if (!composing.current && !isComposing(event.nativeEvent) && slash.onKeyDown(event)) return;
    if (event.key === '@' && mayDrive && !composing.current && !isComposing(event.nativeEvent)
      && !event.ctrlKey && !event.metaKey && /(?:^|\s)$/.test(text.slice(0, event.currentTarget.selectionStart))) {
      event.preventDefault();
      setReference({ kind: 'file', query: '' });
      return;
    }
    if (event.key === 'Escape' && empty && turn !== null) {
      event.preventDefault();
      stop.escape();
      return;
    }
    if (event.key === 'ArrowUp' && empty) {
      (instance?.root.current ?? document).querySelector<HTMLButtonElement>('[data-queued] button[aria-label]')?.focus();
      return;
    }
    if (event.key !== 'Enter' || composing.current || isComposing(event.nativeEvent)) return;
    const mod = event.metaKey || event.ctrlKey;
    if (event.altKey || event.shiftKey && !mod) return;
    event.preventDefault();
    if (mod && event.shiftKey) send('interrupt');
    else if (mod) send('steer');
    else send('queue');
  };

  // What is written goes to a side thread instead, and the turn is left alone: for whoever may drive the thread, where side threads are allowed.
  const aside = useAskOnSide(threadId, { onAccepted: (_reply, question) => composerWriter.clearAcceptedAside(threadId, question.text) });
  const asideWaiting = aside.unconfirmed;
  const modeRefusal = sideRefusal(thread, t);
  const asideRefusal = modeRefusal !== false ? modeRefusal : aside.isPending ? t('side.ask.sending') : asideWaiting ? t('side.ask.retry')
    : parts.length > 0 || uploads.blocked || pastes.blocked || fileDrops.blocked ? t('side.unavailable.attachments') : false;
  const mayAsk = mayDrive && asideRefusal === false;
  const askOnSide = () => {
    if (empty || tooLong || !mayAsk) return;
    const words = text;
    aside.mutate({ text: words, toTurn: null });
  };
  const insertReference = (words: string) => {
    const next = insertAtSelection(text, words, box.current);
    setText(next.text);
    requestAnimationFrame(() => { box.current?.focus(); box.current?.setSelectionRange(next.caret, next.caret); });
  };

  if (!mayDrive) return <div className="flex flex-col gap-2"><ReadOnlyComposerControls />{fileDrops.progress}<UploadChips queue={{ ...uploads, retryReason: t('composer.row.readOnly') }} /><UploadChips queue={{ ...pastes.queue, paste: undefined, retryReason: t('composer.row.readOnly'), rows: pastes.queue.rows.map(({ replaces: _replaces, ...row }) => row), remove: (uploadId) => {
    const pending = pastes.queue.rows.find((row) => row.id === uploadId);
    const index = pending?.replaces === undefined ? -1 : parts.findIndex((part) => part.type === 'pasted_text' && pasteKey(part) === pending.replaces);
    if (index >= 0) removePart(index);
    void pastes.queue.remove(uploadId);
  } }} /></div>;
  return (
    <div className="relative flex flex-col gap-2" data-composer>
      {slash.list}
      <input ref={fileInput} type="file" multiple hidden aria-label={t('uploads.choose')} onChange={(event) => { if (event.target.files !== null) uploads.add(event.target.files); event.target.value = ''; }} />
      <UploadChips queue={uploads} />
      {fileDrops.progress}
      <UploadChips queue={pastes.queue} />
      {needsPasteFacts && !pasteFactsReady && (pasteFactsError === null ? <span role="status" className="text-sm text-text-muted">{t('uploads.readingPasteDelivery')}</span> : <ErrorAlert error={pasteFactsError} action={<Button size="sm" onClick={() => { void Promise.all([parametersState.refetch(), settings.refetch(), contextState.refetch()]); }}>{t('common.tryAgain')}</Button>} />)}
      <AttachmentChips parts={parts} onRemove={removePart} imageIncompatible={imagesUnsupported} imageReason={buildOff.includes('input_images') ? featureReason('input_images') : undefined} onChangeModel={buildOff.includes('input_images') ? undefined : () => setControl({ open: true, section: null })}
        readPaste={pastes.read} pastedIdentity={pastes.identity} onPasteAsText={pastes.asText} onPasteDirty={pastes.markDirty} editText={pastes.editText} editing={pastes.editing} pasteQueue={pastes.queue} onPasteDelivery={pastes.delivery}
        onEditPaste={(index, words) => { const part = parts[index]; if (part?.type === 'pasted_text') pastes.edit(part, words); }}
        pasteMode={(part) => part.delivery !== undefined && part.delivery !== null || pasteFactsReady && (pasteEstimateKnown || inlineShare === 0 || inlineShare === 100) ? pasteDelivery.get(part) : undefined} pasteTokens={(part) => pastedPartTokens(part, pasteEstimate)}
        destination={(part) => part.type === 'pasted_text' && pasteDelivery.get(part) !== false ? null : part.type === 'pasted_text' || part.type === 'file' ? `→ ${part.path}${part.type === 'file' && part.blob === null ? '' : ` · ${t('uploads.whenSent')}`}` : part.type === 'image' ? `→ .orch/attachments/${part.name} · ${t('uploads.whenSent')}` : null} />
      <DropTarget label={t('uploads.target.thread')} countLabel={(values) => t('uploads.target.threadCount', values)} onDrop={(data) => {
        const reference = droppedReference(data);
        if (reference !== null) insertReference(reference);
        else if (carriesFiles(data)) fileDrops.read(data, t('uploads.target.thread'), (files) => uploads.add(files));
      }}>
      <div
        data-control-boundary
        className="flex flex-col gap-1 rounded-lg border border-control-border bg-surface p-2 focus-within:border-control-selected"
      >
        <Textarea
          ref={box}
          label={t('composer.label')}
          hideLabel
          rows={1}
          placeholder={t('composer.placeholder', { agent: agent === '' ? t('composer.theAgent') : agent })}
          value={text}
          disabled={!mayDrive}
          onChange={(event) => setText(event.target.value)}
          onKeyDown={onKeyDown}
          {...slash.boxProps}
          onCompositionStart={() => (composing.current = true)}
          onCompositionEnd={() => (composing.current = false)}
          onPaste={(event) => {
            if (carriesFiles(event.clipboardData)) {
              event.preventDefault();
              fileDrops.read(event.clipboardData, t('uploads.target.thread'), (files) => uploads.add(files));
            } else if (pastes.paste(event.clipboardData.getData('text/plain'))) event.preventDefault();
          }}
          className="max-h-(--composer-max-height) resize-none border-0 bg-transparent px-2 py-1 shadow-none focus-visible:outline-none"
          fieldClassName="w-full"
        />
        <ComposerControls thread={thread} readOnly={!mayDrive}
          notices={<DraftMark status={draft} />}
          attachment={<AttachButton onPress={() => fileInput.current?.click()} disabled={!mayDrive} onFile={() => setReference({ kind: 'file', query: '' })} onDocument={() => setReference({ kind: 'document', query: '' })} fileReason={fileReason} />}
          model={<ModelEffort thread={thread} variant="composer" compact readOnly={!mayDrive} open={control.open} onOpenChange={(open) => setControl((held) => ({ open, section: open ? held.section : null }))} section={control.section} />}
          delivery={<DeliveryControl state={state} stop={stop} turnId={turn?.id ?? null} mayDrive={mayDrive} empty={empty} tooLong={tooLong} attachmentsWaiting={attachmentsWaiting} onSend={send} onAskOnSide={thread.side_of === null && sideReadingAvailable(thread) ? askOnSide : undefined} sideUnavailable={asideRefusal} />}
          plan={planOffered || planUnavailable !== undefined ? <PlanSwitch on={planning} onChange={(on) => composerWriter.setPlanning(threadId, on)} unavailable={planUnavailable} /> : undefined}
        />
        {active && live && <p className="px-2 text-xs text-text-muted" data-delivery-hint>{t(noSteer ? 'composer.runningQueueHint' : isApplePlatform() ? 'composer.runningHintMac' : 'composer.runningHint')}</p>}
        {((maxChars !== null && text.length >= maxChars * COUNTER_FROM) || stop.armed || stop.stop.error !== null) && <div className="flex flex-wrap items-center gap-2" aria-live="polite">
          {maxChars !== null && text.length >= maxChars * COUNTER_FROM && (
            <span className={cx('text-xs tabular-nums', tooLong ? 'text-danger-text' : 'text-text-muted')} role="status">
              {t('composer.counter', { count: text.length, max: maxChars })}
            </span>
          )}
          <StopFeedback stop={stop} />
        </div>}
      </div>
      </DropTarget>
      {slash.problem}
      {refusal !== undefined && refusal !== null && <ErrorAlert error={refusal} />}
      {asideWaiting ? <div className="flex flex-wrap items-center gap-2 text-sm text-text-muted"><p className="min-w-0 flex-1" role="status">{t('side.ask.unconfirmedDraft')}</p><Button size="sm" pending={aside.isPending} onClick={() => { if (aside.variables !== undefined && !aside.isPending) aside.mutate(aside.variables); }}>{t('side.ask.retry')}</Button></div> : aside.isPending ? <p role="status" className="text-xs text-text-muted">{t('side.ask.sending')}</p> : aside.error !== null && <ErrorAlert error={aside.error} />}
      {reference !== null && <ReferencePicker thread={thread} initialKind={reference.kind} initialQuery={reference.query}
        onFile={(path) => addToMessage(threadId, { kind: 'file', path })} onDocument={(file) => uploads.add([file])}
        onClose={() => { setReference(null); box.current?.focus(); }} />}
      {limits.data !== undefined && prompt !== null && (
        <LimitPrompt
          threadId={threadId}
          open
          onOpenChange={(open) => !open && setPrompt(null)}
          limits={limits.data}
          pending={false}
          onSend={(bypass) => {
            const delivery = prompt;
            setPrompt(null);
            if (empty || attachmentsWaiting) return;
            const words = text;
            const content = [...(words === '' ? [] : [{ type: 'text' as const, text: words }]), ...parts];
            setText('');
            composerWriter.setParts(threadId, []);
            submitInput(threadId, content, delivery, bypass, undefined, planning);
            onSent?.();
          }}
        />
      )}
    </div>
  );
}
