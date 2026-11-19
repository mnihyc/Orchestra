/** Shared CodeView component (content): one view for a file of a workspace, a file a document holds, and a difference.
 *
 * A text shown and edited in a small editor of the kind developers use.
 *
 * Highlighting for the language it is given, line numbers, folding, matching brackets, find and
 * replace with patterns, go to a line, several cursors, and the switches for wrapping and for
 * showing whitespace. A text of two megabytes is as quick as one of two kilobytes, since only the
 * lines in view are drawn. The text is the editor's own while it is edited: the owner is told
 * whether it differs from what it gave (so that a tab can say "unsaved") and asks for the text
 * when it needs it, which is when it saves, and nothing is copied on every key. The editor and the
 * language it uses are fetched when a file is first opened, and not before.
 */
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { bracketMatching, foldGutter, foldKeymap, HighlightStyle, indentOnInput, syntaxHighlighting } from '@codemirror/language';
import { highlightSelectionMatches, openSearchPanel, search, searchKeymap } from '@codemirror/search';
import { Compartment, EditorState, type Extension, StateEffect, StateField, Transaction, type Text } from '@codemirror/state';
import {
  crosshairCursor,
  Decoration,
  type DecorationSet,
  drawSelection,
  dropCursor,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  highlightWhitespace,
  keymap,
  lineNumbers,
  rectangularSelection,
} from '@codemirror/view';
import { tags } from '@lezer/highlight';
import { type Ref, useEffect, useImperativeHandle, useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { cx } from '@/ui/cx';
import { carriesFiles, carriesReference } from '@/lib/drop';

/** Where the cursor is, and what is selected. */
export interface CursorInfo {
  line: number;
  column: number;
  /** The lines the selection covers, from one to the other (the same when there is no selection or it is on one line). */
  fromLine: number;
  toLine: number;
  /** Whether anything is selected. */
  selected: boolean;
  /** The line endings of the text: what a status line says of it. */
  endings: 'LF' | 'CRLF' | 'mixed' | 'none';
}

export interface CodeViewHandle {
  /** The text as it stands now. */
  getText: () => string;
  /** Puts other text in place of the text, as an edit that can be undone: what was kept for the person is put back this way. */
  setText: (text: string) => void;
  /** The selected text, or an empty string. */
  getSelection: () => { text: string; fromLine: number; toLine: number };
  /** Says that the text as it stands is the one the owner holds now: it is no longer different. */
  markSaved: () => void;
  focus: () => void;
  /** Brings a line into view and lights it for a moment. */
  goToLine: (line: number) => void;
  /** Lights lines that changed (the agent's writing) until they are marked again. */
  markLines: (lines: readonly number[]) => void;
  /** Opens the editor's find and replace. */
  find: () => void;
}

export interface CodeViewProps {
  /** The text to show. The editor's text is replaced when `version` changes, never otherwise. */
  text: string;
  /** Names this text: the hash of the version it is, so that a newer version replaces what is shown. */
  version: string;
  /** Brings the parser of the language the text is written in; `null` for text. */
  language?: (() => Promise<Extension>) | null;
  /** The key of the language: when it changes the parser is fetched again. */
  languageKey?: string | null;
  readOnly?: boolean;
  /** What the editor is, for a screen reader. */
  label: string;
  wrap?: boolean;
  whitespace?: boolean;
  /** Text typed on top of the version `version` and not saved: put over it when that version is shown, as an edit that can be undone. */
  overlay?: { version: string; text: string } | null;
  /** Lines to light, for the version `version`: what the agent changed in it. */
  lit?: { version: string; lines: readonly number[] } | null;
  /** A line to go to when it changes. */
  line?: number | null;
  /** An explicit repeat request can revisit the same line without replacing the editor or its draft. */
  lineRequest?: number | undefined;
  /** Called when the text begins to differ from what was given, and when it stops. */
  onDirtyChange?: (dirty: boolean) => void;
  /** Called a moment after the last change, with the editor's text, for what is kept for the person. */
  onSettled?: (text: string) => void;
  /** Reports every edit for small controlled configuration forms. */
  onChange?: (text: string) => void;
  /** Reports a person's edit without allocating the entire text on every key. */
  onEdit?: (dirty: boolean) => void;
  /** Flushes the final unsaved text when a draft editor is closed. */
  flushOnUnmount?: boolean;
  /** `Mod+S`. */
  onSave?: () => void;
  onCursor?: (info: CursorInfo) => void;
  className?: string;
  ref?: Ref<CodeViewHandle>;
}

/** How long after the last key `onSettled` is called: a pause in typing, not a rhythm of it. */
export const SETTLE_AFTER_MS = 600;
/** How long a line that was gone to stays lit. */
const FLASH_MS = 1_500;

// ---------------------------------------------------------------- how it looks

const highlight = HighlightStyle.define([
  { tag: [tags.keyword, tags.controlKeyword, tags.moduleKeyword, tags.operatorKeyword, tags.definitionKeyword], color: 'var(--syntax-keyword)' },
  { tag: [tags.string, tags.special(tags.string), tags.regexp, tags.character], color: 'var(--syntax-string)' },
  { tag: [tags.number, tags.bool, tags.null, tags.atom, tags.unit], color: 'var(--syntax-number)' },
  { tag: [tags.comment, tags.lineComment, tags.blockComment, tags.docComment], color: 'var(--syntax-comment)', fontStyle: 'italic' },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName), tags.macroName, tags.propertyName], color: 'var(--syntax-function)' },
  { tag: [tags.typeName, tags.className, tags.namespace, tags.standard(tags.typeName), tags.labelName], color: 'var(--syntax-type)' },
  { tag: [tags.tagName, tags.angleBracket], color: 'var(--syntax-tag)' },
  { tag: [tags.attributeName, tags.meta, tags.processingInstruction, tags.annotation, tags.documentMeta], color: 'var(--syntax-meta)' },
  { tag: [tags.operator, tags.punctuation, tags.separator, tags.bracket], color: 'var(--syntax-operator)' },
  { tag: [tags.link, tags.url], color: 'var(--syntax-link)', textDecoration: 'underline' },
  { tag: tags.heading, fontWeight: '600', color: 'var(--syntax-keyword)' },
  { tag: tags.strong, fontWeight: '700' },
  { tag: tags.emphasis, fontStyle: 'italic' },
  { tag: tags.strikethrough, textDecoration: 'line-through' },
  { tag: tags.inserted, color: 'var(--success-text)' },
  { tag: tags.deleted, color: 'var(--danger-text)' },
  { tag: tags.invalid, color: 'var(--syntax-invalid)' },
]);

const theme = EditorView.theme({
  '&': { height: '100%', color: 'var(--text)', backgroundColor: 'var(--surface)', fontSize: 'var(--text-sm)' },
  '&.cm-focused': { outline: 'var(--focus-ring-width) solid var(--focus)', outlineOffset: '-2px' },
  '.cm-scroller': { fontFamily: 'var(--font-mono)', lineHeight: '1.5', overflow: 'auto' },
  '.cm-content': { caretColor: 'var(--text)', padding: '0.25rem 0' },
  '.cm-line': { padding: '0 0.5rem' },
  '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--text)' },
  '.cm-gutters': { backgroundColor: 'var(--bg-subtle)', color: 'var(--text-subtle)', border: 'none', borderRight: '1px solid var(--border)' },
  '.cm-lineNumbers .cm-gutterElement': { minWidth: 'var(--gutter-width)', padding: '0 0.5rem 0 0.25rem' },
  '.cm-activeLine': { backgroundColor: 'color-mix(in srgb, var(--accent) 7%, transparent)' },
  '.cm-activeLineGutter': { backgroundColor: 'var(--bg-muted)', color: 'var(--text)' },
  '&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection': { backgroundColor: 'var(--accent-subtle)' },
  '.cm-selectionMatch': { backgroundColor: 'color-mix(in srgb, var(--warning) 30%, transparent)' },
  '.cm-matchingBracket, .cm-nonmatchingBracket': { backgroundColor: 'color-mix(in srgb, var(--accent) 22%, transparent)', outline: 'none' },
  '.cm-foldPlaceholder': { backgroundColor: 'var(--bg-muted)', color: 'var(--text-muted)', border: '1px solid var(--border)' },
  '.cm-foldGutter .cm-gutterElement': { cursor: 'pointer', color: 'var(--text-subtle)' },
  '.cm-specialChar': { color: 'var(--syntax-invalid)' },
  '.cm-highlightSpace': { backgroundImage: 'radial-gradient(circle at 50% 55%, var(--text-subtle) 11%, transparent 24%)' },
  '.cm-highlightTab': { backgroundSize: 'auto 100%', backgroundImage: 'linear-gradient(to right, transparent 90%, var(--text-subtle) 90%)' },
  '.cm-agent-line': { backgroundColor: 'color-mix(in srgb, var(--success) 16%, transparent)', boxShadow: 'inset 3px 0 0 var(--success)' },
  '.cm-flash-line': { backgroundColor: 'color-mix(in srgb, var(--warning) 28%, transparent)' },
  '.cm-panels': { backgroundColor: 'var(--bg-subtle)', color: 'var(--text)', borderColor: 'var(--border)' },
  '.cm-panels-top': { borderBottom: '1px solid var(--border)' },
  '.cm-panel.cm-search': { padding: '0.25rem 1.75rem 0.25rem 0.5rem' },
  '.cm-panel.cm-search label': { color: 'var(--text-muted)', fontSize: 'var(--text-sm)' },
  '.cm-textfield': { backgroundColor: 'var(--surface)', color: 'var(--text)', border: '1px solid var(--border-strong)', borderRadius: '0.25rem', padding: '0.125rem 0.375rem' },
  '.cm-button': {
    backgroundImage: 'none',
    backgroundColor: 'var(--bg-muted)',
    color: 'var(--text)',
    border: '1px solid var(--border-strong)',
    borderRadius: '0.25rem',
    padding: '0.125rem 0.5rem',
  },
  '.cm-searchMatch': { backgroundColor: 'color-mix(in srgb, var(--warning) 30%, transparent)', outline: '1px solid var(--warning)' },
  '.cm-searchMatch.cm-searchMatch-selected': { backgroundColor: 'color-mix(in srgb, var(--accent) 35%, transparent)' },
  '.cm-tooltip': { backgroundColor: 'var(--surface-raised)', color: 'var(--text)', border: '1px solid var(--border)' },
});

// ---------------------------------------------------------------- lines that are lit

const markLines = StateEffect.define<readonly number[]>();
const flashLine = StateEffect.define<number | null>();

/** The lines the agent changed, lit until marked again; mapped through edits so that they stay on their text. */
const markedField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(marks, transaction) {
    let next = marks.map(transaction.changes);
    for (const effect of transaction.effects) {
      if (effect.is(markLines)) {
        const doc = transaction.state.doc;
        next = Decoration.set(
          effect.value
            .filter((line) => line >= 1 && line <= doc.lines)
            .map((line) => Decoration.line({ class: 'cm-agent-line' }).range(doc.line(line).from)),
          true,
        );
      }
    }
    return next;
  },
  provide: (field) => EditorView.decorations.from(field),
});

const flashField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(flash, transaction) {
    let next = flash.map(transaction.changes);
    for (const effect of transaction.effects) {
      if (effect.is(flashLine)) {
        const doc = transaction.state.doc;
        const line = effect.value;
        next = line === null || line < 1 || line > doc.lines ? Decoration.none : Decoration.set([Decoration.line({ class: 'cm-flash-line' }).range(doc.line(line).from)]);
      }
    }
    return next;
  },
  provide: (field) => EditorView.decorations.from(field),
});

/** The line endings of a text, from the first thousand lines of it. */
function endingsOf(doc: Text): CursorInfo['endings'] {
  const text = doc.sliceString(0, Math.min(doc.length, 20_000));
  const crlf = (text.match(/\r\n/g) ?? []).length;
  const lf = (text.match(/\n/g) ?? []).length - crlf;
  if (crlf === 0 && lf === 0) return 'none';
  if (crlf > 0 && lf > 0) return 'mixed';
  return crlf > 0 ? 'CRLF' : 'LF';
}

export function CodeView({
  text,
  version,
  language = null,
  languageKey = null,
  readOnly = false,
  label,
  wrap = false,
  whitespace = false,
  overlay = null,
  lit = null,
  line = null,
  lineRequest,
  onDirtyChange,
  onSettled,
  onChange,
  onEdit,
  flushOnUnmount = false,
  onSave,
  onCursor,
  className,
  ref,
}: CodeViewProps) {
  const { t } = useTranslation();
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const base = useRef<Text | null>(null);
  const dirty = useRef(false);
  const flash = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const settle = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  /** The latest of what the owner gave, read by the editor's listeners, which are made once. */
  const latest = useRef({ onDirtyChange, onSettled, onChange, onEdit, flushOnUnmount, onSave, onCursor, text, version, overlay, lit });
  useEffect(() => {
    latest.current = { onDirtyChange, onSettled, onChange, onEdit, flushOnUnmount, onSave, onCursor, text, version, overlay, lit };
  });

  const languageSlot = useMemo(() => new Compartment(), []);
  const readSlot = useMemo(() => new Compartment(), []);
  const wrapSlot = useMemo(() => new Compartment(), []);
  const spaceSlot = useMemo(() => new Compartment(), []);
  const labelSlot = useMemo(() => new Compartment(), []);

  const phrases = useMemo(
    () => ({
      Find: t('files.code.find'),
      Replace: t('files.code.replace'),
      next: t('files.code.next'),
      previous: t('files.code.previous'),
      all: t('files.code.all'),
      'match case': t('files.code.matchCase'),
      regexp: t('files.code.regexp'),
      'by word': t('files.code.byWord'),
      replace: t('files.code.replaceOne'),
      'replace all': t('files.code.replaceAll'),
      close: t('files.code.closeSearch'),
      'Go to line': t('files.code.goToLine'),
      go: t('files.code.go'),
      'current match': t('files.code.currentMatch'),
      'on line': t('files.code.onLine'),
    }),
    [t],
  );

  // The editor is made once. What changes afterwards goes in through its slots.
  useEffect(() => {
    const parent = host.current;
    if (parent === null) return;
    const now = latest.current;
    const state = EditorState.create({
      doc: now.text,
      extensions: [
        EditorView.contentAttributes.of({ 'data-content-role': 'source' }),
        lineNumbers(),
        highlightActiveLineGutter(),
        highlightSpecialChars(),
        history(),
        foldGutter(),
        drawSelection(),
        dropCursor(),
        EditorView.domEventHandlers({
          dragover: (event) => {
            if (event.dataTransfer !== null && (carriesFiles(event.dataTransfer) || carriesReference(event.dataTransfer))) { event.preventDefault(); return true; }
            return false;
          },
          drop: (event) => {
            if (event.dataTransfer !== null && (carriesFiles(event.dataTransfer) || carriesReference(event.dataTransfer))) { event.preventDefault(); event.stopPropagation(); return true; }
            return false;
          },
        }),
        EditorState.allowMultipleSelections.of(true),
        indentOnInput(),
        syntaxHighlighting(highlight),
        bracketMatching(),
        rectangularSelection(),
        crosshairCursor(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        search({ top: true }),
        keymap.of([
          {
            key: 'Mod-s',
            preventDefault: true,
            run: () => {
              latest.current.onSave?.();
              return true;
            },
          },
          ...defaultKeymap,
          ...searchKeymap,
          ...historyKeymap,
          ...foldKeymap,
          indentWithTab,
        ]),
        markedField,
        flashField,
        theme,
        languageSlot.of([]),
        readSlot.of([]),
        wrapSlot.of([]),
        spaceSlot.of([]),
        labelSlot.of([]),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            latest.current.onChange?.(update.state.doc.toString());
            const differs = base.current === null || !update.state.doc.eq(base.current);
            if (update.transactions.some((transaction) => transaction.annotation(Transaction.userEvent) !== undefined)) latest.current.onEdit?.(differs);
            if (differs !== dirty.current) {
              dirty.current = differs;
              latest.current.onDirtyChange?.(differs);
            }
            clearTimeout(settle.current);
            settle.current = setTimeout(() => latest.current.onSettled?.(update.state.doc.toString()), SETTLE_AFTER_MS);
          }
          if (update.selectionSet || update.docChanged) {
            const main = update.state.selection.main;
            const at = update.state.doc.lineAt(main.head);
            const from = update.state.doc.lineAt(main.from);
            const to = update.state.doc.lineAt(main.to);
            latest.current.onCursor?.({
              line: at.number,
              column: main.head - at.from + 1,
              fromLine: from.number,
              toLine: to.number,
              selected: !main.empty,
              endings: endingsOf(update.state.doc),
            });
          }
        }),
      ],
    });
    const created = new EditorView({ state, parent });
    created.scrollDOM.dataset.scrollAllow = 'code';
    view.current = created;
    base.current = created.state.doc;
    dirty.current = false;
    return () => {
      if (latest.current.flushOnUnmount && dirty.current) latest.current.onSettled?.(created.state.doc.toString());
      clearTimeout(flash.current);
      clearTimeout(settle.current);
      created.destroy();
      view.current = null;
    };
  }, [languageSlot, readSlot, wrapSlot, spaceSlot, labelSlot]);

  // The switches and the words round the editor.
  useEffect(() => {
    view.current?.dispatch({
      effects: [
        readSlot.reconfigure([EditorState.readOnly.of(readOnly), EditorView.contentAttributes.of(readOnly ? { 'aria-readonly': 'true' } : {})]),
        wrapSlot.reconfigure(wrap ? EditorView.lineWrapping : []),
        spaceSlot.reconfigure(whitespace ? highlightWhitespace() : []),
        labelSlot.reconfigure([EditorView.contentAttributes.of({ 'aria-label': label }), EditorState.phrases.of(phrases)]),
      ],
    });
  }, [readOnly, wrap, whitespace, label, phrases, readSlot, wrapSlot, spaceSlot, labelSlot]);

  // The language: its parser is fetched now, and applied when it arrives, if it is still the one wanted.
  useEffect(() => {
    let wanted = true;
    if (language === null) {
      view.current?.dispatch({ effects: languageSlot.reconfigure([]) });
      return;
    }
    void language().then(
      (extension) => {
        if (wanted) view.current?.dispatch({ effects: languageSlot.reconfigure(extension) });
      },
      () => undefined,
    );
    return () => {
      wanted = false;
    };
    // The key stands for the loader: it is a new function at every drawing.
  }, [languageKey, languageSlot]); // eslint-disable-line react-hooks/exhaustive-deps

  // A newer version of the text replaces what is shown, and becomes what "different" is measured from.
  useEffect(() => {
    const editor = view.current;
    if (editor === null) return;
    if (editor.state.doc.toString() !== text) {
      editor.dispatch({
        changes: { from: 0, to: editor.state.doc.length, insert: text },
        annotations: Transaction.addToHistory.of(false),
      });
    }
    base.current = editor.state.doc;
    if (dirty.current) {
      dirty.current = false;
      latest.current.onDirtyChange?.(false);
    }
    // What was typed on this version and not saved, and what the agent changed in it, goes over it: both are of this version alone.
    const now = latest.current;
    if (now.overlay?.version === version && editor.state.doc.toString() !== now.overlay.text) {
      editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: now.overlay.text } });
    }
    if (now.lit?.version === version) editor.dispatch({ effects: markLines.of(now.lit.lines) });
  }, [version]); // eslint-disable-line react-hooks/exhaustive-deps

  const goTo = (target: number) => {
    const editor = view.current;
    if (editor === null) return;
    const lines = editor.state.doc.lines;
    const number = Math.max(1, Math.min(target, lines));
    const at = editor.state.doc.line(number);
    editor.dispatch({
      selection: { anchor: at.from },
      effects: [EditorView.scrollIntoView(at.from, { y: 'center' }), flashLine.of(number)],
    });
    clearTimeout(flash.current);
    flash.current = setTimeout(() => view.current?.dispatch({ effects: flashLine.of(null) }), FLASH_MS);
  };

  useEffect(() => {
    if (line !== null && line >= 1) goTo(line);
  }, [line, version, lineRequest]);

  useImperativeHandle(
    ref,
    () => ({
      getText: () => view.current?.state.doc.toString() ?? '',
      setText: (next) => {
        const editor = view.current;
        if (editor === null) return;
        editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: next } });
      },
      getSelection: () => {
        const editor = view.current;
        if (editor === null) return { text: '', fromLine: 1, toLine: 1 };
        const { from, to } = editor.state.selection.main;
        return {
          text: editor.state.sliceDoc(from, to),
          fromLine: editor.state.doc.lineAt(from).number,
          toLine: editor.state.doc.lineAt(to).number,
        };
      },
      markSaved: () => {
        const editor = view.current;
        if (editor === null) return;
        base.current = editor.state.doc;
        if (dirty.current) {
          dirty.current = false;
          latest.current.onDirtyChange?.(false);
        }
      },
      focus: () => view.current?.focus(),
      goToLine: goTo,
      markLines: (lines) => view.current?.dispatch({ effects: markLines.of(lines) }),
      find: () => {
        const editor = view.current;
        if (editor === null) return;
        openSearchPanel(editor);
      },
    }),
    // `goTo` reads the editor through a ref.
    [],
  );

  return <div ref={host} data-code-view className={cx('min-h-(--code-min-height) overflow-hidden', className)} />;
}
