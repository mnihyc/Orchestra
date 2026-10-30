/** The block menu behind a slash, and the habits of Markdown that turn into blocks while typing.
 *
 * `/` opens the list of blocks where the caret is and narrows it as the person types; `@` names a
 * person, `[[` links a place or a document, `:` and two letters pick an emoji. What people type from
 * habit becomes the block it means (`# `, `- `, `1. `, `[] `, `> `, three backticks, `---`, `$$`), and
 * nothing on the page is ever shown as Markdown or stored as it.
 */
import { Extension, InputRule } from '@tiptap/core';
import type { Editor, Range } from '@tiptap/core';
import { PluginKey, Selection, TextSelection } from '@tiptap/pm/state';
import Suggestion, { type SuggestionKeyDownProps, type SuggestionProps } from '@tiptap/suggestion';
import {
  AtSign,
  Braces,
  CheckSquare,
  ChevronRight,
  Columns2,
  FileText,
  Film,
  Heading1,
  Heading2,
  Heading3,
  Image,
  Link2,
  List,
  ListOrdered,
  Minus,
  Music,
  Pilcrow,
  Quote,
  Scissors,
  Sigma,
  Smile,
  SquareCode,
  Table2,
  TriangleAlert,
  Workflow,
  type LucideIcon,
} from 'lucide-react';
import { useEffect, useRef } from 'react';
import { createPortal } from 'react-dom';
import { FLOATING_GAP, floatingClasses, floatingPoint } from '@/ui/Popover';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { ThreadIdentity } from '@/features/people/ThreadIdentity';
import { create } from 'zustand';

import { containerAt, containerStartingAt, insertAfter, isTextContent, turnInto } from '@/features/documents/commands';
import { askAddress, askDocumentFiles } from '@/features/documents/Media';
import { blockToJson, emptyTable, newId, nodeOf } from '@/features/documents/schema';
import type { UserRef } from '@/generated/api-types';
import type { DocumentMentionCandidate, DocumentMentionPage } from '@/api/people';
import { i18n } from '@/i18n';
import { choiceOption, offeredChoice } from '@/i18n/choices';
import { Choice } from '@/ui/Choice';
import { cx } from '@/ui/cx';
import { ChoiceInspection, type CardController } from '@/ui/DetailCard';
import { PersonCard } from '@/ui/PersonCard';
import { UserChip } from '@/ui/UserChip';
import type { PersonThing } from '@/features/people/personContext';
import { type EmojiEntry, emojiName, findEmoji } from '@/ui/emoji-data';

// ---------------------------------------------------------------- what the menu offers

export interface BlockChoice {
  id: string;
  /** The words it is found by, besides its name. */
  keywords: string;
  icon: LucideIcon;
}

const choice = (id: string, icon: LucideIcon, keywords: string): BlockChoice => ({ id, icon, keywords });

/** Every block the menu offers, in the order it is shown. */
export const BLOCK_CHOICES: readonly BlockChoice[] = [
  choice('paragraph', Pilcrow, 'text paragraph plain'),
  choice('heading1', Heading1, 'heading title h1 #'),
  choice('heading2', Heading2, 'heading h2 ##'),
  choice('heading3', Heading3, 'heading h3 ###'),
  choice('bullet', List, 'bulleted list unordered ul -'),
  choice('numbered', ListOrdered, 'numbered list ordered ol 1.'),
  choice('todo', CheckSquare, 'todo task checkbox checklist []'),
  choice('code', SquareCode, 'code snippet ```'),
  choice('quote', Quote, 'quote blockquote >'),
  choice('callout', TriangleAlert, 'callout note warning info'),
  choice('table', Table2, 'table grid rows'),
  choice('divider', Minus, 'divider line rule ---'),
  choice('toggle', ChevronRight, 'toggle fold collapse details'),
  choice('math', Sigma, 'formula math latex equation'),
  choice('diagram', Workflow, 'diagram mermaid chart flow'),
  choice('columns2', Columns2, 'columns two 2 layout'),
  choice('columns3', Columns2, 'columns three 3 layout'),
  choice('columns4', Columns2, 'columns four 4 layout'),
  choice('page_break', Scissors, 'page break print'),
  choice('image', Image, 'image picture photo screenshot'),
  choice('file', FileText, 'file attachment upload'),
  choice('video', Film, 'film video movie'),
  choice('audio', Music, 'sound audio music'),
  choice('mention', AtSign, 'mention person @'),
  choice('link', Link2, 'link thread topic project document reference'),
  choice('emoji', Smile, 'emoji smile :'),
  choice('heading4', Heading3, 'heading h4'),
  choice('heading5', Heading3, 'heading h5'),
  choice('heading6', Heading3, 'heading h6'),
];

/** One block choice uses the same paired copy in the slash list and the block's menu. */
export function blockOption(id: string) {
  const heading = /^heading(\d)$/.exec(id);
  if (heading !== null) return choiceOption(i18n.t, 'documentBlock.heading', { level: heading[1] ?? '1' });
  const columns = /^columns(\d)$/.exec(id);
  if (columns !== null) return choiceOption(i18n.t, 'documentBlock.columns', { count: Number(columns[1]) });
  return offeredChoice(i18n.t, 'documentBlock', id) ?? choiceOption(i18n.t, 'dynamic.value', { name: i18n.t(`documents.blocks.${id}` as 'documents.blocks.paragraph'), context: i18n.t('documents.menu.block') });
}

/** The plain name remains the word matched by typeahead. */
export function choiceLabel(id: string): string { return blockOption(id).name; }

/** The block menu's choices that match what was typed. */
export function matchChoices(query: string): BlockChoice[] {
  const q = query.trim().toLowerCase();
  if (q === '') return BLOCK_CHOICES.filter((one) => !/^heading[4-6]$/.test(one.id));
  return BLOCK_CHOICES.filter((one) => choiceLabel(one.id).toLowerCase().includes(q) || one.keywords.includes(q) || one.id.includes(q));
}

// ---------------------------------------------------------------- making a block

const paragraphJson = () => ({ type: 'block', attrs: { id: newId() }, content: [{ type: 'paragraph' }] });

/** The editor's JSON for a new block of the kind a choice names. */
function blockFor(id: string): ReturnType<typeof blockToJson> | null {
  switch (id) {
    case 'math':
      return blockToJson({ id: newId(), type: 'math', props: { source: '' }, content: [] });
    case 'diagram':
      return blockToJson({ id: newId(), type: 'diagram', props: { source: '' }, content: [] });
    case 'divider':
      return blockToJson({ id: newId(), type: 'divider', props: {}, content: [] });
    case 'page_break':
      return blockToJson({ id: newId(), type: 'page_break', props: {}, content: [] });
    case 'table':
      return blockToJson({ id: newId(), type: 'table', props: { header_row: true }, content: emptyTable(3, 3) });
    default: {
      const columns = /^columns(\d)$/.exec(id);
      if (columns === null) return null;
      const count = Number(columns[1]);
      return {
        type: 'columns',
        attrs: { id: newId() },
        content: Array.from({ length: count }, () => ({ type: 'column', attrs: { id: newId(), share: null }, content: [paragraphJson()] })),
      };
    }
  }
}

/** Whether the block the caret is in is a paragraph with nothing in it and nothing under it: a new block may take its place. */
function emptyParagraphAt(editor: Editor): number | null {
  const { selection, doc } = editor.state;
  const at = containerAt(doc, selection.from);
  if (at?.node.type.name !== 'block' || at.node.childCount > 1) return null;
  const content = at.node.child(0);
  return content.type.name === 'paragraph' && content.content.size === 0 ? at.pos : null;
}

/** Puts a block where the caret is: in place of an empty paragraph, or after the block the caret is in. */
function put(editor: Editor, json: ReturnType<typeof blockToJson>): void {
  // The editor's own schema: a node made by another one does not fit into the page.
  const schema = editor.schema;
  const node = schema.nodeFromJSON(json);
  const { state } = editor;
  const empty = emptyParagraphAt(editor);
  const at = containerAt(state.doc, state.selection.from);
  if (at === null) return;
  const tr = state.tr;
  let start = at.pos;
  if (empty !== null) tr.replaceWith(empty, empty + at.node.nodeSize, node);
  else {
    start = at.pos + at.node.nodeSize;
    tr.insert(start, node);
  }
  // A block that holds no text, with nothing after it where it stands (the page, a list's children, a column), needs a line after it to write in.
  const after = tr.doc.resolve(start + node.nodeSize);
  if (node.type.name !== 'block' && after.nodeAfter === null) tr.insert(start + node.nodeSize, schema.nodeFromJSON(paragraphJson()));
  tr.setSelection(Selection.near(tr.doc.resolve(start + 1), 1));
  editor.view.dispatch(tr.scrollIntoView());
  editor.view.focus();
}

/** Does what a choice of the menu says, with the text the person typed to find it already taken away. */
export function applyChoice(editor: Editor, id: string): void {
  const { state } = editor;
  const at = containerAt(state.doc, state.selection.from);
  if (at === null) return;
  const heading = /^heading(\d)$/.exec(id);
  const text: Record<string, [string, Record<string, unknown>]> = {
    paragraph: ['paragraph', {}],
    bullet: ['bullet', {}],
    numbered: ['numbered', {}],
    todo: ['todo', { checked: false }],
    toggle: ['toggle', {}],
    quote: ['quote', {}],
    callout: ['callout', { tone: 'info' }],
    code: ['codeBlock', { language: '' }],
  };
  const turned = heading !== null ? (['heading', { level: Number(heading[1]) }] as [string, Record<string, unknown>]) : text[id];
  if (turned !== undefined) {
    const tr = state.tr;
    if (turnInto(tr, at.pos, turned[0], turned[1])) {
      tr.setSelection(TextSelection.near(tr.doc.resolve(at.pos + 2)));
      editor.view.dispatch(tr.scrollIntoView());
    }
    editor.view.focus();
    return;
  }
  switch (id) {
    case 'image':
    case 'file':
      askDocumentFiles(editor, id === 'image');
      return;
    case 'video':
    case 'audio':
      askAddress(id, (url) => {
        put(editor, blockToJson({ id: newId(), type: id, props: { url }, content: [] }));
      });
      return;
    case 'mention':
      editor.chain().focus().insertContent('@').run();
      return;
    case 'link':
      editor.chain().focus().insertContent('[[').run();
      return;
    case 'emoji':
      editor.chain().focus().insertContent(':').run();
      return;
    default: {
      const json = blockFor(id);
      if (json !== null) put(editor, json);
    }
  }
}

/** Opens the menu in a new line after the block at `pos`: what the "+" beside a block does. */
export function openMenuAfter(editor: Editor, pos: number): void {
  const block = editor.schema.nodeFromJSON(paragraphJson());
  const tr = insertAfter(editor.state, pos, block);
  if (tr === null) return;
  editor.view.dispatch(tr);
  editor.view.focus();
  editor.commands.insertContent('/');
}

// ---------------------------------------------------------------- the list that opens at the caret

export interface SuggestItem {
  threadId?: string;
  id: string;
  label: string;
  description?: string;
  hint?: string;
  icon?: LucideIcon;
  avatar?: UserRef;
  glyph?: string;
  disabled?: boolean;
}

interface SuggestState {
  open: boolean;
  kind: 'block' | 'person' | 'emoji' | 'link';
  items: SuggestItem[];
  index: number;
  rect: { left: number; top: number; bottom: number } | null;
  pick: ((index: number) => void) | null;
  thing?: PersonThing | undefined;
  restore?: (() => void) | undefined;
}

const closed: SuggestState = { open: false, kind: 'block', items: [], index: 0, rect: null, pick: null };
export const useSuggest = create<SuggestState>(() => closed);
const personInspectors = new Map<string, CardController>();

/** Where the people, the places and the documents a line can name are, as the page last knew them. */
export const sources = {
  people: [] as UserRef[],
  places: [] as { id: string; kind: 'node' | 'document'; name: string; hint: string; threadId?: string }[],
};

interface Source<T> {
  kind: SuggestState['kind'];
  char: string;
  key: string;
  items: (query: string) => T[] | Promise<T[]>;
  show: (item: T) => SuggestItem;
  run: (editor: Editor, range: Range, item: T) => void;
  /** A continuation is an option that loads more choices without changing the editor. */
  more?: { is: (item: T) => boolean; load: (item: T) => Promise<T[]>; replace?: boolean };
  /** Whether the list stays open for what has been typed after the character that opened it. */
  when?: (query: string) => boolean;
}

function suggestion<T>(editor: Editor, source: Source<T>) {
  const key = new PluginKey(source.key);
  return Suggestion<T, T>({
    editor,
    char: source.char,
    pluginKey: key,
    allowSpaces: source.kind === 'link' || source.kind === 'block',
    items: ({ query }) => source.items(query),
    allow: ({ state, range }) => {
      const parent = state.doc.resolve(range.from).parent;
      if (!isTextContent(parent) && parent.type.name !== 'tableCell') return false;
      // What was typed after the character that opens the list; a leaf in the line counts as one character.
      const typed = state.doc.textBetween(range.from, range.to, '', '\u0000').slice(source.char.length);
      return source.when === undefined || source.when(typed);
    },
    // The words typed to find it are taken from where they stand now: the list's own range is as it was when it was last drawn, and a change above it since then has moved it.
    command: ({ editor: at, range, props }) => {
      const now = (key.getState(at.state) as { active?: boolean; range?: Range } | undefined) ?? {};
      source.run(at, now.active === true && now.range !== undefined ? now.range : range, props);
    },
    render: () => {
      const show = (props: SuggestionProps<T, T>) => {
        const box = props.clientRect?.();
        const doc = (editor.storage as unknown as { orchDoc?: { id?: string } }).orchDoc;
        const at = editor.state.selection.$from;
        let blockId: string | undefined;
        for (let depth = at.depth; depth >= 0; depth -= 1) {
          const parent = at.node(depth);
          if ((parent.type.name === 'block' || parent.type.name === 'leafBlock') && typeof parent.attrs.id === 'string') { blockId = parent.attrs.id; break; }
        }
        const thing: PersonThing | undefined = source.kind !== 'person' || doc?.id === undefined ? undefined : { kind: 'document', id: doc.id, ...(blockId === undefined ? {} : { anchor: { block: blockId } }) };
        let choices = props.items;
        let loading = false;
        const pick = (index: number) => {
          const item = choices[index];
          if (item === undefined) return;
          const more = source.more;
          if (more?.is(item) !== true) { props.command(item); return; }
          if (loading) return;
          loading = true;
          const rows = choices.map(source.show);
          rows[index] = { ...source.show(item), label: i18n.t('common.loading'), disabled: true };
          useSuggest.setState({ items: rows });
          void more.load(item).then((next) => {
            if (!useSuggest.getState().open || useSuggest.getState().pick !== pick) return;
            choices = more.replace === true ? next : [...choices.slice(0, index), ...next, ...choices.slice(index + 1)];
            useSuggest.setState({ items: choices.map(source.show), index: more.replace === true ? Math.max(0, next.findIndex((one) => !more.is(one))) : Math.max(0, Math.min(index, choices.length - 1)) });
          }).catch(() => {
            if (!useSuggest.getState().open || useSuggest.getState().pick !== pick) return;
            const failed = choices.map(source.show);
            failed[index] = { ...source.show(item), label: i18n.t('people.quick.loadFailed') };
            useSuggest.setState({ items: failed });
          }).finally(() => { loading = false; });
        };
        useSuggest.setState({
          open: true,
          kind: source.kind,
          items: props.items.map(source.show),
          index: 0,
          rect: box === null || box === undefined ? null : { left: box.left, top: box.top, bottom: box.bottom },
          pick,
          thing,
          restore: () => { editor.commands.focus(); },
        });
      };
      return {
        onStart: show,
        onUpdate: show,
        onKeyDown: ({ event }: SuggestionKeyDownProps) => {
          const state = useSuggest.getState();
          if (!state.open) return false;
          if (event.key === 'ArrowRight' && state.kind === 'person') {
            const item = state.items[state.index];
            const inspector = item?.avatar === undefined ? undefined : personInspectors.get(item.id);
            if (inspector !== undefined) { event.preventDefault(); inspector.keep(); return true; }
          }
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            const count = state.items.length;
            if (count === 0) return true;
            useSuggest.setState({ index: (state.index + (event.key === 'ArrowDown' ? 1 : count - 1)) % count });
            return true;
          }
          if (event.key === 'Enter' || event.key === 'Tab') {
            if (state.items.length === 0) return false;
            state.pick?.(state.index);
            return true;
          }
          if (event.key === 'Escape') {
            useSuggest.setState(closed);
            return false;
          }
          return false;
        },
        onExit: () => useSuggest.setState(closed),
      };
    },
  });
}

const block: Source<BlockChoice> = {
  kind: 'block',
  char: '/',
  key: 'orchBlockMenu',
  items: (query) => (query.startsWith(' ') ? [] : matchChoices(query)),
  // A name of two words ("heading 2", "page break") is typed whole; the list goes when a space is typed and nothing is left that it could mean, so that a slash in running text is only a slash.
  when: (query) => !query.startsWith(' ') && (!/\s/.test(query) || matchChoices(query).length > 0),
  show: (one) => ({ id: one.id, ...blockOption(one.id), icon: one.icon }),
  run: (editor, range, one) => {
    editor.chain().focus().deleteRange(range).run();
    applyChoice(editor, one.id);
  },
};

type PersonSuggestion = { kind: 'person'; candidate: DocumentMentionCandidate } | { kind: 'more'; query: string; cursor: string | null; failed: boolean; first?: boolean };

function personPage(page: DocumentMentionPage, query: string, first = false): PersonSuggestion[] {
  return [...(first ? [{ kind: 'more' as const, query, cursor: null, failed: false, first: true }] : []), ...page.items.map((candidate): PersonSuggestion => ({ kind: 'person', candidate })),
    ...(page.next_cursor === null ? [] : [{ kind: 'more' as const, query, cursor: page.next_cursor, failed: false }])];
}

const people: Source<PersonSuggestion> = {
  kind: 'person',
  char: '@',
  key: 'orchMention',
  items: (query) => {
    const q = query.trim().toLowerCase();
    return sources.people.filter((one) => one.display_name.toLowerCase().includes(q) || one.username.toLowerCase().includes(q)).slice(0, 8).map((user): PersonSuggestion => ({ kind: 'person', candidate: { user, can_view: true } }));
  },
  show: (one) => one.kind === 'more' ? { id: one.first === true ? 'first-people' : 'more-people', label: i18n.t(one.failed ? 'people.quick.loadFailed' : one.first === true ? 'people.quick.firstPeople' : 'people.quick.morePeople') } : ({ id: one.candidate.user.id, label: one.candidate.user.display_name, hint: `@${one.candidate.user.username}${one.candidate.can_view ? '' : ` · ${i18n.t('people.quick.noAccess')}`}`, avatar: one.candidate.user }),
  run: (editor, range, one) => {
    if (one.kind !== 'person') return;
    const { candidate } = one;
    editor
      .chain()
      .focus()
      .insertContentAt(range, [{ type: 'mention', attrs: { user_id: candidate.user.id, name: candidate.user.display_name, username: candidate.user.username, user: candidate.user, can_view: candidate.can_view, prompt: true } }, { type: 'text', text: ' ' }])
      .run();
  },
};

type Place = (typeof sources.places)[number];
const places: Source<Place> = {
  kind: 'link',
  char: '[[',
  key: 'orchLink',
  items: (query) => {
    const q = query.trim().toLowerCase();
    return sources.places.filter((one) => one.name.toLowerCase().includes(q)).slice(0, 8);
  },
  show: (one) => ({ id: one.id, label: one.name, hint: one.hint, threadId: one.threadId, icon: one.kind === 'document' ? FileText : Braces }),
  run: (editor, range, one) => {
    editor
      .chain()
      .focus()
      .insertContentAt(range, [{ type: 'ref', attrs: { kind: one.kind, id: one.id, name: one.name } }, { type: 'text', text: ' ' }])
      .run();
  },
};

/** How many emoji the list under a colon shows. */
const EMOJI_SHOWN = 8;

const emoji: Source<EmojiEntry> = {
  kind: 'emoji',
  char: ':',
  key: 'orchEmoji',
  // `:` and two letters opens the list.
  items: (query) => (/^[a-z0-9_+-]{2,}$/i.test(query) ? findEmoji(query).slice(0, EMOJI_SHOWN) : []),
  show: (one) => ({ id: one[0], label: emojiName(one), glyph: one[0] }),
  run: (editor, range, one) => {
    editor.chain().focus().insertContentAt(range, `${one[0]} `).run();
  },
};

/** The lists that open at the caret: blocks, people, places and documents, emoji. */
export const Suggestions = Extension.create<{ people: ((query: string, cursor: string | null) => Promise<DocumentMentionPage>) | null }>({
  name: 'orchSuggestions',
  addOptions() { return {people:null}; },
  // An open list answers the arrow keys, Enter and Escape before the editor's own keys do.
  priority: 1100,
  addProseMirrorPlugins() {
    const currentPeople = this.options.people;
    const offered: Source<PersonSuggestion> = currentPeople === null ? people : { ...people,
      items: async (query) => {
        try { return personPage(await currentPeople(query, null), query); }
        catch { return [{ kind: 'more', query, cursor: null, failed: true }]; }
      },
      more: { replace: true, is: (item) => item.kind === 'more', load: async (item) => item.kind === 'more' ? personPage(await currentPeople(item.query, item.cursor), item.query, item.cursor !== null) : [] },
    };
    return [suggestion(this.editor, block), suggestion(this.editor, offered), suggestion(this.editor, places), suggestion(this.editor, emoji)];
  },
});

// ---------------------------------------------------------------- Markdown as a habit of typing

/** A habit of Markdown that turns the paragraph being typed in into another block. */
function habit(find: RegExp, make: (match: RegExpMatchArray) => { node: string; attrs?: Record<string, unknown> } | { leaf: 'divider' | 'math' }): InputRule {
  return new InputRule({
    find,
    handler: ({ state, range, match }) => {
      const $from = state.doc.resolve(range.from);
      const at = containerAt(state.doc, range.from);
      // Only a paragraph turns: a list item that starts with a dash stays what it is.
      if (at?.node.type.name !== 'block' || $from.parent.type.name !== 'paragraph') return null;
      const tr = state.tr;
      tr.delete(range.from, range.to);
      const made = make(match);
      if ('leaf' in made) {
        const schema = state.schema;
        const leaf = made.leaf === 'divider' ? blockToJson({ id: newId(), type: 'divider', props: {}, content: [] }) : blockToJson({ id: newId(), type: 'math', props: { source: '' }, content: [] });
        const container = containerStartingAt(tr.doc, at.pos);
        if (container === null) return null;
        tr.replaceWith(at.pos, at.pos + container.node.nodeSize, [schema.nodeFromJSON(leaf), schema.nodeFromJSON(paragraphJson())]);
        const next = at.pos + schema.nodeFromJSON(leaf).nodeSize;
        tr.setSelection(TextSelection.near(tr.doc.resolve(next + 2)));
        return;
      }
      if (!turnInto(tr, at.pos, made.node, made.attrs ?? {})) return null;
      tr.setSelection(TextSelection.near(tr.doc.resolve(at.pos + 2)));
    },
  });
}

/** What people type from habit and what it turns into. */
export const Habits = Extension.create({
  name: 'orchHabits',
  addInputRules() {
    return [
      habit(/^(#{1,6})\s$/, (match) => ({ node: 'heading', attrs: { level: (match[1] ?? '#').length } })),
      habit(/^[-+*]\s$/, () => ({ node: 'bullet' })),
      habit(/^(\d+)[.)]\s$/, (match) => ({ node: 'numbered', attrs: { start: Number(match[1]) === 1 ? null : Number(match[1]) } })),
      habit(/^\[([ x])?\]\s$/, (match) => ({ node: 'todo', attrs: { checked: match[1] === 'x' } })),
      habit(/^>\s$/, () => ({ node: 'quote' })),
      habit(/^```([\w+-]*)\s$/, (match) => ({ node: 'codeBlock', attrs: { language: match[1] ?? '' } })),
      habit(/^---$/, () => ({ leaf: 'divider' })),
      habit(/^\$\$$/, () => ({ leaf: 'math' })),
      // A formula in a line is drawn when the second dollar sign is typed.
      new InputRule({
        find: /\$([^$\s][^$]*)\$$/,
        handler: ({ state, range, match }) => {
          const source = match[1];
          if (source === undefined) return null;
          state.tr.replaceWith(range.from, range.to, nodeOf(state.schema, 'inlineMath').create({ source }));
        },
      }),
    ];
  },
});

// ---------------------------------------------------------------- the list, drawn

/** The list that opens at the caret. It is drawn once for the page, and the editor keeps its focus. */
/** The identifier of the list that opens at the caret. */
const SUGGEST_LIST = 'suggest-list';

export function SuggestMenu() {
  const { t } = useTranslation();
  const { open, items, index, rect, pick, kind, thing, restore } = useSuggest();
  const list = useRef<HTMLDivElement>(null);
  const pointer = useRef({ x: -1, y: -1 });
  useEffect(() => {
    list.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' });
  }, [index, items]);
  // The page stays where the caret is, so the list is told to a reader of the screen through it: which entry is the current one.
  const current = items[index]?.id;
  useEffect(() => {
    const page = document.querySelector<HTMLElement>('.doc-editor');
    if (page === null || !open || current === undefined) return;
    page.setAttribute('aria-controls', SUGGEST_LIST);
    page.setAttribute('aria-activedescendant', `suggest-${current}`);
    return () => {
      page.removeAttribute('aria-controls');
      page.removeAttribute('aria-activedescendant');
    };
  }, [open, current]);
  if (!open || rect === null || typeof document === 'undefined') return null;
  const below = rect.bottom + FLOATING_GAP;
  // Prefer the side with more room for a long suggestion list; the shared bounds handle its size.
  const above = rect.top - FLOATING_GAP > window.innerHeight - below;
  return createPortal(
    <div className="fixed z-(--layer-dropdown) [--floating-width:var(--doc-menu)] [--floating-height:var(--doc-menu-height)] w-(--doc-menu)" style={floatingPoint(rect.left, above ? rect.top - FLOATING_GAP : below, above)}><ChoiceInspection><div
      ref={list}
      id={SUGGEST_LIST}
      role="listbox"
      // A list that scrolls can be reached by the keyboard; while it is open the editor keeps the caret and answers the arrows, Enter and Tab for it.
      tabIndex={0}
      aria-label={t(`documents.menu.${kind}` as 'documents.menu.block')}
      className={`w-full overflow-y-auto rounded-lg border border-border bg-surface-raised p-1 shadow-lg ${floatingClasses}`}
      onMouseDown={(event) => event.preventDefault()}
    >
      {items.length === 0 ? (
        <EmptyState compact className="px-3" title={t('documents.menu.empty')} />
      ) : (
        items.map((item, at) => {
          const Icon = item.icon;
          return (
            <Choice presentation="picker" key={item.id} name={item.avatar === undefined ? item.label : <UserChip user={item.avatar} card={false} size={24} />} description={item.description ?? item.hint} compact className="w-full">{(content, control) => {
              const option = <span
              {...control}
              id={`suggest-${item.id}`}
              role="option"
              aria-selected={at === index}
              aria-disabled={item.disabled === true ? true : undefined}
              className={cx(
                'flex w-full cursor-pointer items-center gap-3 rounded-md px-2 py-1 text-sm',
                at === index ? 'bg-accent-subtle text-text' : 'text-text',
              )}
              onMouseMove={(event) => {
                // The current entry follows the pointer when it moves, not when a list opens under one that is resting.
                if (pointer.current.x === event.clientX && pointer.current.y === event.clientY) return;
                pointer.current = { x: event.clientX, y: event.clientY };
                useSuggest.setState({ index: at });
              }}
              onClick={() => pick?.(at)}
            >
              {Icon !== undefined && <Icon aria-hidden className="size-4 shrink-0 text-text-muted" />}
              {item.glyph !== undefined && <span aria-hidden className="w-4 shrink-0 text-center">{item.glyph}</span>}
              <span className="min-w-0 flex-1">{content}{item.threadId !== undefined && <ThreadIdentity id={item.threadId} />}</span>
            </span>;
              return item.avatar === undefined ? option : <PersonCard user={item.avatar} choice active={at === index} thing={thing} controller={(value) => { if (value === null) personInspectors.delete(item.id); else personInspectors.set(item.id, value); }} returnFocus={restore}>{option}</PersonCard>;
            }}</Choice>
          );
        })
      )}
    </div></ChoiceInspection></div>,
    document.body,
  );
}
