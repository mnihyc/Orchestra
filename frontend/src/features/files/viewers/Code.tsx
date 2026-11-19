/** Code and text: highlighting, line numbers, folding, find and replace, go to a line, several cursors.
 *
 * The viewer that takes any text and edits it where it is shown. It reads the version that was
 * brought, shows it in the code view, saves on the version it was read at, keeps what was typed and not
 * saved on the device, follows the file while the tab holds nothing unsaved (the lines that changed
 * are marked), and when the file changes under unsaved text shows both and lets the person decide.
 * A text longer than is read is shown to that point, read-only, with how much there is.
 */
import { useViewSaveGuard } from '@/features/workbench/CloseViews';
import { useQueryClient } from '@tanstack/react-query';
import { AlignLeft, Link2, MessageSquarePlus, Save, Search, Undo2, WrapText } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { currentOf, writeFile } from '@/api/workspace';
import { useFormatContext } from '@/lib/appearance';
import { formatDateTime } from '@/lib/time';
import { Button } from '@/ui/Button';
import { CodeView, type CodeViewHandle, type CursorInfo } from '@/ui/CodeView';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { InlineAlert } from '@/ui/InlineAlert';
import { Combobox } from '@/ui/Combobox';
import { SkeletonRows } from '@/ui/Skeleton';
import { copyText } from '@/ui/clipboard';
import { environmentFileReference } from '@/features/people/references';
import { sourceHasReference, sourceReference, useFileSource } from '../source';

import { addToMessage } from '../AddToMessage';
import { changedLines } from '../AgentEdits';
import { invalidateHead, useFileText } from '../content';
import { Unavailable } from '../Unavailable';
import { ChangedBar, forgetUnsaved, type KeptText, keepUnsaved, ReadOnlyNote, readUnsaved, useUnsavedMark } from '../Editing';
import { keepFileTab } from '../FileTabs';
import { languageById, languageOf, LANGUAGES } from '../languages';
import { PREVIEW_BYTES, type ViewerProps } from '../viewers';
import { TooLargeNote } from './TooLarge';

const PLAIN = 'plain';

interface Shown {
  text: string;
  /** Names the version it is; a new name replaces what the editor holds. */
  version: string;
}

export default function CodeViewer({ door, path, head, line, lineRequest, tabId, whyReadOnly, thread, onText }: ViewerProps) {
  const source = useFileSource();
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const userId = useStore(client.session, (state) => state.userId);
  const format = useFormatContext();
  const loaded = useFileText(door, path, head.brought);
  const arrived = loaded.text;
  const code = useRef<CodeViewHandle>(null);

  const [shown, setShown] = useState<Shown | null>(null);
  /** The hash of the version the editor's text is based on: what a save names as the version it read. */
  const [base, setBase] = useState<string | null>(null);
  /** The version that arrived last and was taken in. */
  const [seen, setSeen] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  const dirtyNow = useRef(false);
  const [conflict, setConflict] = useState(false);
  const [comparing, setComparing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<unknown>(null);
  const [cursor, setCursor] = useState<CursorInfo | null>(null);
  const [wrap, setWrap] = useState(false);
  const [whitespace, setWhitespace] = useState(false);
  const [chosenLanguage, setChosenLanguage] = useState<string | null>(null);
  const [languageQuery, setLanguageQuery] = useState('');
  const [restored, setRestored] = useState<(KeptText & { version: string }) | null>(null);
  const [marks, setMarks] = useState<{ version: string; lines: number[] } | null>(null);
  const [discarding, setDiscarding] = useState(false);
  const [discards, setDiscards] = useState(0);

  const cut = arrived?.cut ?? false;
  const readOnly = whyReadOnly !== null || cut;
  const reason = whyReadOnly ?? (cut && arrived !== null ? ({ kind: 'too_large', shown: PREVIEW_BYTES, total: arrived.total } as const) : null);

  useUnsavedMark(tabId, dirty);

  // A version that arrived is taken in as it does: shown the first time, followed when nothing is unsaved,
  // and set against the person's text, with the choice put to them, when something is.
  if (arrived !== null && arrived.hash !== seen) {
    setSeen(arrived.hash);
    if (shown === null) {
      setShown({ text: arrived.text, version: arrived.hash });
      setBase(arrived.hash);
      const kept = readOnly ? null : readUnsaved(userId, door.environmentId, path);
      if (kept !== null && kept.text !== arrived.text) {
        setRestored({ ...kept, version: arrived.hash });
        // It was typed on another version than the one that is there now.
        if (kept.base !== null && kept.base !== arrived.hash) setConflict(true);
      }
    } else if (arrived.hash !== base) {
      if (!dirty) {
        setMarks({ version: arrived.hash, lines: changedLines(shown.text, arrived.text) });
        setShown({ text: arrived.text, version: arrived.hash });
        setBase(arrived.hash);
      } else {
        setConflict(true);
      }
    }
  }

  const language = useMemo(() => {
    if (shown === null) return null;
    if (chosenLanguage === PLAIN) return null;
    if (chosenLanguage !== null) return languageById(chosenLanguage);
    return languageOf(head.stat.name, shown.text.slice(0, 200).split('\n', 1)[0] ?? null);
  }, [chosenLanguage, head.stat.name, shown]);

  const onDirtyChange = useCallback(
    (now: boolean) => {
      dirtyNow.current = now;
      setDirty(now);
      if (now) keepFileTab(tabId);
    },
    [tabId],
  );

  const baseNow = useRef<string | null>(null);
  useEffect(() => {
    baseNow.current = base;
  }, [base]);

  const onSettled = useCallback(
    (text: string) => {
      onText?.(text);
      if (dirtyNow.current) keepUnsaved(userId, door.environmentId, path, { base: baseNow.current, text });
      else forgetUnsaved(userId, door.environmentId, path);
    },
    [userId, door.environmentId, path, onText],
  );

  const save = useCallback(async () => {
    const editor = code.current;
    if (editor === null || readOnly || saving || !dirtyNow.current) return;
    // Saving from the comparison writes on the version that is there now.
    const expected = comparing && arrived !== null ? arrived.hash : base;
    setSaving(true);
    setSaveError(null);
    try {
      const stat = await writeFile(door, path, editor.getText(), expected);
      setBase(stat.hash ?? expected);
      editor.markSaved();
      forgetUnsaved(userId, door.environmentId, path);
      setConflict(false);
      setComparing(false);
      setRestored(null);
    } catch (error) {
      if (currentOf(error) !== null) {
        // Not the version that was read: nothing was written, and what is there now is asked for.
        setConflict(true);
        void invalidateHead(queryClient, door, path);
      } else {
        setSaveError(error);
      }
    } finally {
      setSaving(false);
    }
  }, [readOnly, saving, comparing, arrived, base, door, path, userId, queryClient]);
  useViewSaveGuard(tabId, {
    save: async () => { await save(); return !dirtyNow.current; },
    discard: () => { forgetUnsaved(userId, door.environmentId, path); dirtyNow.current = false; setDirty(false); },
  });


  /** Puts the version that is there now in the editor's place, and what the person typed is gone. */
  const takeTheirs = () => {
    if (arrived === null) return;
    setBase(arrived.hash);
    forgetUnsaved(userId, door.environmentId, path);
    setShown({ text: arrived.text, version: `${arrived.hash}:${discards + 1}` });
    setDiscards(discards + 1);
    setConflict(false);
    setComparing(false);
    setRestored(null);
    setDiscarding(false);
  };

  const keepMine = () => {
    if (arrived !== null) setBase(arrived.hash);
    setConflict(false);
    setComparing(false);
  };

  const addSelection = () => {
    if (thread === null) return;
    const selection = code.current?.getSelection();
    if (selection !== undefined && selection.text !== '') {
      addToMessage(thread.id, { kind: 'lines', path, fromLine: selection.fromLine, toLine: selection.toLine, text: selection.text });
    } else {
      addToMessage(thread.id, { kind: 'file', path });
    }
  };

  // ---------------------------------------------------------------- what is drawn

  if (head.brought.state === 'unavailable') {
    return <Unavailable reason={head.brought.reason} />;
  }
  if (loaded.error !== null) {
    return <ErrorState compact className="h-full" error={loaded.error} onRetry={() => void invalidateHead(queryClient, door, path)} />;
  }
  if (shown === null) return <SkeletonRows rows={8} />;

  const languageOptions = [{ value: PLAIN, label: t('files.code.plainText'), text: t('files.code.plainText') },
    ...LANGUAGES.map((entry) => ({ value: entry.id, label: entry.name, text: entry.name }))];
  const selectedLanguage = chosenLanguage ?? language?.id ?? PLAIN;

  return (
    <div className="@container flex h-full min-h-0 flex-col" data-viewer="code">
      {reason !== null && <ReadOnlyNote reason={reason} />}
      {reason?.kind === 'too_large' && <TooLargeNote shown={PREVIEW_BYTES} total={reason.total} />}
      {restored !== null && !conflict && (
        <InlineAlert tone="info" className="m-2">
          {t('files.edit.restored', { when: formatDateTime(restored.at, format) })}
        </InlineAlert>
      )}
      {conflict && (
        <ChangedBar
          by={null}
          comparing={comparing}
          pending={arrived === null || arrived.hash === base}
          onCompare={() => setComparing(true)}
          onKeepMine={keepMine}
          onTakeTheirs={takeTheirs}
        />
      )}
      {saveError !== null && <ErrorAlert error={saveError} className="m-2" />}
      <div className="flex flex-wrap items-center gap-1 border-b border-border px-2 py-1">
        {!readOnly && (
          <>
            <Button size="sm" variant="primary" icon={<Save aria-hidden />} pending={saving} disabled={!dirty} onClick={() => void save()}>
              {t('files.edit.save')}
            </Button>
            <IconButton size="sm" label={t('files.edit.discard')} icon={<Undo2 aria-hidden />} disabled={!dirty} onClick={() => setDiscarding(true)} />
          </>
        )}
        <span className="flex-1" />
        <IconButton size="sm" label={t('people.reference.copy')} blocked={source !== null && !sourceHasReference(source)} reason={source !== null && !sourceHasReference(source) ? t('fileMedia.noReference') : undefined} icon={<Link2 aria-hidden />} onClick={() => {
          if (source !== null && !sourceHasReference(source)) return;
          const at = cursor?.selected === true ? cursor.fromLine : cursor?.line ?? line ?? 1;
          void copyText(source === null ? environmentFileReference(door.environmentId, path, at) : sourceReference(source, at), t('people.reference.copied'));
        }} />
        <IconButton size="sm" label={t('files.code.find')} shortcut="Mod+F" icon={<Search aria-hidden />} onClick={() => code.current?.find()} />
        <IconButton size="sm" label={t('files.code.wrap')} pressed={wrap} icon={<WrapText aria-hidden />} onClick={() => setWrap((on) => !on)} />
        <IconButton size="sm" label={t('files.code.whitespace')} pressed={whitespace} icon={<AlignLeft aria-hidden />} onClick={() => setWhitespace((on) => !on)} />
        {thread !== null && <IconButton size="sm" label={t('files.addToMessage')} icon={<MessageSquarePlus aria-hidden />} onClick={addSelection} />}
      </div>
      <div className={comparing ? 'grid min-h-0 flex-1 grid-cols-1 grid-rows-2 gap-px bg-border @3xl:grid-cols-2 @3xl:grid-rows-1' : 'min-h-0 flex-1'}>
        <div className="min-h-0 min-w-0 bg-surface">
          <CodeView
            ref={code}
            className="h-full"
            text={shown.text}
            version={shown.version}
            language={language === null ? null : language.load}
            languageKey={language?.id ?? null}
            readOnly={readOnly}
            label={t('files.code.label', { name: head.stat.name })}
            wrap={wrap}
            whitespace={whitespace}
            overlay={restored === null ? null : { version: restored.version, text: restored.text }}
            lit={marks}
            line={line}
            lineRequest={lineRequest}
            onDirtyChange={onDirtyChange}
            onSettled={onSettled}
            onSave={() => void save()}
            onCursor={setCursor}
          />
        </div>
        {comparing && arrived !== null && (
          <div className="flex min-h-0 min-w-0 flex-col bg-surface" data-testid="their-version">
            <p className="border-b border-border px-3 py-1 text-xs font-medium text-text-muted">{t('files.edit.changed.theirs')}</p>
            <CodeView
              className="min-h-0 flex-1"
              text={arrived.text}
              version={arrived.hash}
              language={language === null ? null : language.load}
              languageKey={language?.id ?? null}
              readOnly
              label={t('files.edit.changed.theirsLabel', { name: head.stat.name })}
              wrap={wrap}
              whitespace={whitespace}
            />
          </div>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-border px-3 py-1 text-xs text-text-muted" data-testid="code-status">
        <span className="tabular-nums">{t('files.code.position', { line: cursor?.line ?? 1, column: cursor?.column ?? 1 })}</span>
        {cursor?.selected === true && <span>{t('files.code.selected', { count: cursor.toLine - cursor.fromLine + 1 })}</span>}
        <span className="min-w-0 flex-1" />
        <Combobox
          size="sm"
          label={t('files.code.language')}
          hideLabel
          query={languageQuery}
          onQueryChange={setLanguageQuery}
          empty={t('files.code.noLanguage')}
          displayValue={languageOptions.find((option) => option.value === selectedLanguage)?.text ?? t('files.code.plainText')}
          onSelect={(option) => { setChosenLanguage(option.value); setLanguageQuery(''); }}
          groups={[{ id: 'languages', label: t('files.code.language'),
            options: languageOptions.filter((option) => option.text.toLocaleLowerCase().includes(languageQuery.trim().toLocaleLowerCase())) }]}
          fieldClassName="w-(--field-narrow)"
        />
        {cursor !== null && cursor.endings !== 'none' && <span>{cursor.endings}</span>}
        {dirty && <span className="font-medium text-text">{t('files.edit.unsaved')}</span>}
      </div>
      <ConfirmDialog
        open={discarding}
        onOpenChange={setDiscarding}
        title={t('files.edit.discardTitle', { name: head.stat.name })}
        confirmLabel={t('files.edit.discard')}
        danger
        onConfirm={takeTheirs}
      >
        {t('files.edit.discardWhat')}
      </ConfirmDialog>
    </div>
  );
}
