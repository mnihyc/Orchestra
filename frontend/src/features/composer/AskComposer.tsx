/** The shared composer adapts to a private, read-only conversation without an environment. */
import { AtSign, Send, Square } from 'lucide-react';
import { type KeyboardEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { useQuery } from '@tanstack/react-query';
import { client } from '@/api/client';
import { askModelsQuery } from '@/api/ask';
import { persistComposerDraft, useAttachmentParts, useBox, useComposerStore, useDraftSync } from './Draft';
import { askSendIntent, completeAskSendIntent } from './askDrafts';
import { AttachButton, AttachmentChips, UploadChips } from './Attachments';
import { ComposerControlRow, ComposerFrame } from './ComposerFrame';
import { DraftMark } from './DraftMark';
import { DropTarget } from './DropTarget';
import { useFileDrops } from './FileDrops';
import { AskModelChoice } from '@/features/ask/ModelChoice';
import { ResourcePicker } from '@/features/ask/ResourcePicker';
import { useViewShown } from '@/features/workbench/viewOpening';
import { useViewInstance } from '@/features/workbench/instance';
import { useViewSaveGuard } from '@/features/workbench/CloseViews';
import { useWorkbench } from '@/features/workbench/store';
import type { AskModel, ContentPart } from '@/generated/api-types';
import { carriesFiles, droppedReference, insertAtSelection } from '@/lib/drop';
import { isComposing } from '@/lib/shortcuts';
import { attachToThread, releaseUpload } from '@/lib/uploads';
import { useUploads } from '@/lib/useUploads';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Textarea } from '@/ui/Textarea';

export interface AskComposerProps {
  scope: string; root: string; draftKey?: string; conversationId?: string | null;
  active: boolean; disabled: boolean; canStop?: boolean; model: AskModel | null;
  ensure: () => Promise<string>;
  send: (content: ContentPart[], clientId: string) => Promise<void>;
  stop: () => Promise<void>;
  onModel: (value: AskModel | null) => void;
}
interface Draft { text: string; parts: ContentPart[] }
interface Attached { part: ContentPart; key: string }
interface ComposerAction { kind: 'allocate' | 'send' | 'stop'; run: () => Promise<void> }
interface ComposerFailure { error: unknown; action: ComposerAction }

export function AskComposer({ scope, root, draftKey: givenKey, conversationId, active, disabled, canStop = !disabled, model, ensure, send, stop, onModel }: AskComposerProps) {
  const { t } = useTranslation();
  const shown = useViewShown();
  const reader = useStore(client.session, (state) => `${state.userId ?? ''}:${state.readerVersion}`);
  const draftKey = givenKey ?? `ask:${reader}:${scope}`;
  const { text } = useBox(draftKey), parts = useAttachmentParts(draftKey);
  const draft = useMemo(() => ({ text, parts }), [text, parts]);
  const change = (update: (draft: Draft) => Draft) => useComposerStore.setState((before) => {
    const next = update({ text: before.text[draftKey] ?? '', parts: before.parts[draftKey] ?? [] });
    return { text: { ...before.text, [draftKey]: next.text }, parts: { ...before.parts, [draftKey]: next.parts } };
  });
  const [pendingKeys, setPendingKeys] = useState<ReadonlySet<string>>(new Set());
  const [failures, setFailures] = useState<Record<string, ComposerFailure | null>>({});
  const pending = pendingKeys.has(draftKey), failure = failures[draftKey] ?? null;
  const setPending = useCallback((value: boolean) => setPendingKeys((before) => { const next = new Set(before); if (value) next.add(draftKey); else next.delete(draftKey); return next; }), [draftKey]);
  const setFailure = useCallback((failure: ComposerFailure | null) => setFailures((before) => ({ ...before, [draftKey]: failure })), [draftKey]);
  const latestAction = useRef(new Map<string, ComposerAction>()), pendingActions = useRef(new Set<string>());
  const runAction = useCallback(async (action: ComposerAction, { background = false, retry = false } = {}) => {
    if ((action.kind === 'stop' ? !canStop : disabled) || pendingActions.current.has(draftKey)) return;
    latestAction.current.set(draftKey, action);
    if (!background) { pendingActions.current.add(draftKey); setPending(true); }
    if (!retry) setFailure(null);
    try {
      await action.run();
      if (latestAction.current.get(draftKey) === action) setFailure(null);
    } catch (error) {
      // A late background allocation answer cannot replace a later explicit Send/Stop failure.
      if (latestAction.current.get(draftKey) === action) setFailure({ error, action });
    } finally {
      if (!background) { pendingActions.current.delete(draftKey); setPending(false); }
    }
  }, [canStop, disabled, draftKey, setFailure, setPending]);
  const [draftDirty, setDraftDirty] = useState(false);
  const [reference, setReference] = useState(false);
  const box = useRef<HTMLTextAreaElement>(null), input = useRef<HTMLInputElement>(null);
  const [tags, setTags] = useState<ReadonlyMap<string, string>>(new Map());
  // A queue remains mounted while the page changes root; each job captures its own private destination.
  const transfers = useUploads<Attached>('ask-composer', async () => { throw new Error('An attachment needs its conversation'); }, ({ part, key }) => {
    useComposerStore.setState((before) => ({ parts: { ...before.parts, [key]: [...before.parts[key] ?? [], part] } }));
  });
  const rows = transfers.rows.filter((row) => tags.get(row.id) === draftKey);
  const uploads = { rows, blocked: rows.length > 0, add: (files: Iterable<File>) => addFiles(files), remove: transfers.remove, retry: transfers.retry };
  const addFiles = (files: Iterable<File>) => {
    const key = draftKey;
    const added: string[] = [];
    // Allocate before transfer starts. No model is run, and every later retry keeps this destination.
    let knownId: string | null = conversationId ?? null;
    const allocate = () => ensure().then((id) => { knownId = id; return { id }; }, (error: unknown) => ({ error }));
    let destination = knownId === null ? allocate() : Promise.resolve({ id: knownId });
    for (const file of files) {
      let attempted = false;
      const ids = transfers.add([file], undefined, async (blob, signal) => {
        if (knownId === null && attempted) destination = allocate();
        attempted = true;
        const target = await destination; if ('error' in target) throw target.error;
        const part = await attachToThread(target.id, blob, signal);
        await persistComposerDraft(target.id, key, part);
        return { part, key };
      });
      added.push(...ids);
    }
    if (added.length > 0) setTags((before) => {
      const next = new Map(before);
      for (const id of added) next.set(id, key);
      return next;
    });
  };
  const drops = useFileDrops('ask-composer');
  const saved = useDraftSync(conversationId ?? '', conversationId != null && !disabled, 'queue', shown, draftKey, setDraftDirty);
  const hasDraft = text !== '' || parts.length > 0 || rows.length > 0;
  const dirty = draftDirty || transfers.rows.length > 0 || conversationId == null && hasDraft;
  const instanceId = useViewInstance()?.id;
  useEffect(() => { if (instanceId !== undefined) { useWorkbench.getState().setUnsaved(instanceId, dirty); return () => useWorkbench.getState().setUnsaved(instanceId, false); } }, [instanceId, dirty]);
  useViewSaveGuard(instanceId, {
    save: async () => { if (transfers.rows.length > 0) throw new Error(t('ask.waitForFiles')); const id = conversationId ?? await ensure(); await persistComposerDraft(id, draftKey); return true; },
    discard: () => { for (const row of transfers.rows) void transfers.remove(row.id); if (draftDirty) change(() => ({ text: '', parts: [] })); },
  });
  useEffect(() => {
    if (conversationId != null || disabled || !hasDraft || latestAction.current.has(draftKey)) return;
    void runAction({ kind: 'allocate', run: async () => { await ensure(); } }, { background: true });
  }, [conversationId, disabled, hasDraft, draftKey, ensure, runAction]);
  useEffect(() => {
    const field = box.current; if (field === null) return;
    const resize = () => { field.style.height = 'auto'; const line = Number.parseFloat(getComputedStyle(field).lineHeight) || 20; field.style.height = `${Math.min(field.scrollHeight, line * 12)}px`; };
    resize(); const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(resize); observer?.observe(field);
    return () => observer?.disconnect();
  }, [text]);
  useEffect(() => {
    if (!dirty) return;
    const warn = (event: BeforeUnloadEvent) => event.preventDefault(); window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  }, [dirty]);
  const imageFacts = useQuery({ ...askModelsQuery(root), enabled: shown && root !== '' && parts.some((part) => part.type === 'image') });
  const effectiveModel = model ?? imageFacts.data?.default_model;
  const imageModel = imageFacts.data?.models.find((option) => option.selection.provider_id === effectiveModel?.provider_id && option.selection.model === effectiveModel?.model);
  const sendNow = async () => {
    if (disabled || pending || active || uploads.blocked || drops.blocked || text.trim() === '' && parts.length === 0) return;
    const content: ContentPart[] = [...(text === '' ? [] : [{ type: 'text' as const, text }]), ...parts];
    const fingerprint = JSON.stringify([content, model]);
    const attempt = askSendIntent(draftKey, fingerprint, (id) => send(content, id));
    await runAction({ kind: 'send', run: async () => {
      await attempt.send();
      useComposerStore.setState((before) => before.text[draftKey] === draft.text && JSON.stringify(before.parts[draftKey] ?? []) === JSON.stringify(draft.parts)
        ? { text: { ...before.text, [draftKey]: '' }, parts: { ...before.parts, [draftKey]: [] } } : before);
      completeAskSendIntent(draftKey, attempt);
    } });
  };
  const insert = (words: string) => {
    const next = insertAtSelection(text, words, box.current); change((draft) => ({ ...draft, text: next.text }));
    requestAnimationFrame(() => { box.current?.focus(); box.current?.setSelectionRange(next.caret, next.caret); });
  };
  const keys = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (isComposing(event.nativeEvent)) return;
    if (event.key === '@' && !event.ctrlKey && !event.metaKey && /(?:^|\s)$/.test(text.slice(0, event.currentTarget.selectionStart))) { event.preventDefault(); setReference(true); }
    if (event.key === 'Enter' && !event.shiftKey && !event.altKey) { event.preventDefault(); void sendNow(); }
  };
  const removePart = (index: number) => {
    const part = parts[index];
    if (part !== undefined && 'blob' in part && part.blob !== null) void releaseUpload(part.blob, conversationId == null ? undefined : { thread: conversationId, path: part.type === 'image' ? part.path ?? `.orch/attachments/${part.name}` : 'path' in part ? part.path : '' }).catch(() => undefined);
    change((draft) => ({ ...draft, parts: draft.parts.filter((_, at) => index !== at) }));
  };
  return <div data-composer data-ask-composer className="flex min-w-0 shrink-0 flex-col gap-2 p-3">
    <input ref={input} type="file" multiple hidden aria-label={t('uploads.choose')} disabled={disabled || pending} onChange={(event) => { if (event.target.files !== null) addFiles(Array.from(event.target.files)); event.target.value = ''; }} />
    <UploadChips queue={{ ...uploads, retryReason: disabled ? t('composer.row.readOnly') : undefined }} />{drops.progress}
    <AttachmentChips parts={parts} onRemove={disabled ? undefined : removePart} destination={(part) => part.type === 'image' && imageModel?.model !== null && imageModel?.model !== undefined && imageModel.model.parameters.input_images !== true ? t('ask.imageReference') : null} />
    <DropTarget disabled={disabled || pending} label={t('uploads.target.thread')} onDrop={(data) => { const ref = droppedReference(data); if (ref !== null) insert(ref); else if (carriesFiles(data)) drops.read(data, t('uploads.target.thread'), addFiles); }}>
      <ComposerFrame>
        <Textarea ref={box} label={t('ask.composer')} hideLabel rows={1} placeholder={t('ask.placeholder')} disabled={disabled || pending} value={text}
          onChange={(event) => change((draft) => ({ ...draft, text: event.target.value }))} onKeyDown={keys}
          onPaste={(event) => { if (carriesFiles(event.clipboardData)) { event.preventDefault(); drops.read(event.clipboardData, t('uploads.target.thread'), addFiles); } }}
          className="max-h-(--composer-max-height) resize-none border-0 bg-transparent px-2 py-1 shadow-none focus-visible:outline-none" fieldClassName="w-full" />
        <ComposerControlRow label={t('composer.row.label')}>
          <AttachButton disabled={disabled || pending} onPress={() => input.current?.click()} />
          <IconButton size="sm" label={t('ask.resources')} disabled={disabled || pending} icon={<AtSign aria-hidden />} onClick={() => setReference(true)} />
          <AskModelChoice root={root} value={model} disabled={disabled || pending} onChange={onModel} />
          <span className="ml-auto flex items-center gap-1"><DraftMark status={saved} />{active ? <Button size="sm" pending={pending} disabled={!canStop && !pending} icon={<Square aria-hidden />} onClick={() => { void runAction({ kind: 'stop', run: stop }); }}>{t('composer.stop')}</Button>
            : <Button size="sm" variant="primary" pending={pending} disabled={disabled || uploads.blocked || drops.blocked || text.trim() === '' && parts.length === 0} icon={<Send aria-hidden />} onClick={() => void sendNow()}>{t('composer.send')}</Button>}</span>
        </ComposerControlRow>
      </ComposerFrame>
    </DropTarget>
    {failure !== null && <ErrorAlert error={failure.error} action={<Button size="sm" pending={pending} disabled={!pending && (failure.action.kind === 'stop' ? !canStop : disabled)} onClick={() => { void runAction(failure.action, { retry: true }); }}>{t('common.tryAgain')}</Button>} />}
    {reference && !disabled && <ResourcePicker root={root} onChoose={insert} onClose={() => { setReference(false); box.current?.focus(); }} />}
  </div>;
}
