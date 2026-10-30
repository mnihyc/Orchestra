/** Paste: text that says Markdown, and a page of rich text, become the blocks the server's reader says they are, where the caret is.
 *
 * There is one reader of Markdown and of HTML, the server's (`POST /documents/convert`), and what it gives back is
 * laid on the page here. Only text that came as text is read as Markdown: rich text from another page is read as
 * HTML, and rich text from this editor has the editor's own way in. A line of words with a bold word or a link in
 * it goes into the line the caret is in; anything with a block in it (a heading, a list, a table, a fence) goes in
 * as blocks, in place of an empty line or after the block the caret is in. Code and table cells take the text as it is.
 */
import { Extension, type JSONContent } from '@tiptap/core';
import type { Fragment, Node as PmNode } from '@tiptap/pm/model';
import { Plugin, PluginKey, Selection, type EditorState, type Transaction } from '@tiptap/pm/state';

import { containerAt, isTextContent } from '@/features/documents/commands';
import { piecesOf, type Piece } from '@/features/documents/pieces';
import { blockToJson, newId } from '@/features/documents/schema';
import { readerWarningText } from '@/features/documents/readerWarnings';
import type { Inline, NewBlock, ReaderWarning } from '@/generated/api-types';
import { i18n } from '@/i18n';

const FENCE = /^ {0,3}(`{3,}|~{3,})/;
const DIVIDER = /^ {0,3}([-*_])(?:\s*\1){2,}\s*$/;
const QUOTE = /^ {0,3}>\s?/;
const DELIMITER = /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/;
/** How long a reading may take before the page says it is waiting for it. */
const SLOW_READING_MS = 400;

/** Whether pasted text says something in Markdown that plain text does not: a block of one of its kinds, or a bold word, a link or code in a line. This only decides whether to ask the reader; it reads nothing. */
export function looksLikeMarkdown(text: string): boolean {
  const lines = text.replace(/\r\n?/g, '\n').split('\n');
  for (const [index, line] of lines.entries()) {
    if (FENCE.test(line) || /^ {0,3}#{1,6}\s+\S/.test(line) || QUOTE.test(line) || /^\s*([-*+]|\d{1,9}[.)])\s+\S/.test(line)) return true;
    if (DIVIDER.test(line) && index > 0) return true;
    if (/^\s*<(details|toggle|columns|column|page-break)\b/i.test(line) || /^ {0,3}<!--\s*orch:block(?:\s|-->)/.test(line)) return true;
    if (line.includes('|') && DELIMITER.test(lines[index + 1] ?? '') && (lines[index + 1] ?? '').includes('-')) return true;
  }
  // A marker is enough to ask the one reader; do not parse destinations here. Excluding opening brackets keeps
  // long unfinished link sequences linear instead of retrying the rest of the text from every opening bracket.
  return /\*\*[^\s*][^*]*\*\*|`[^`\n]+`|\[[^[\]\n]*\]\(|<(?:u|span|br|video|audio|file)(?:\s|\/?>)|\$\$|\$[^\s$][^$\n]*\$/i.test(text);
}

/** What the reader gave back for a paste. */
export interface Converted { blocks: readonly NewBlock[]; warnings: readonly ReaderWarning[] }

/** Asks the server's reader what a pasted text says. */
export type Convert = (format: 'markdown' | 'html', text: string) => Promise<Converted>;

/** What a piece is as the editor's JSON, with the blocks under it. */
function pieceJson(one: Piece): JSONContent {
  const json = blockToJson({ id: newId(), type: one.type, props: one.props, content: one.content });
  if (one.children.length === 0) return json;
  return { ...json, content: [...(json.content ?? []), { type: 'blockGroup', content: one.children.map(pieceJson) }] };
}

/** The pieces as blocks of the state's schema. */
function blocksOf(state: EditorState, pieces: readonly Piece[]): PmNode[] {
  return pieces.map((one) => state.schema.nodeFromJSON(pieceJson(one)));
}

/** The words of one paragraph as nodes of a line, with the spaces that stood around them in what was pasted: between two words they matter. */
function lineOf(state: EditorState, one: Piece, source: string): Fragment {
  const lead = /^[^\S\n]+/.exec(source)?.[0] ?? '';
  const trail = /[^\S\n]+$/.exec(source)?.[0] ?? '';
  const content: Inline[] = [...(lead === '' ? [] : [{ type: 'text' as const, text: lead }]), ...(one.content as Inline[]), ...(trail === '' ? [] : [{ type: 'text' as const, text: trail }])];
  const block = state.schema.nodeFromJSON(blockToJson({ id: newId(), type: 'paragraph', props: {}, content }));
  return block.child(0).content;
}

/** Whether the caret is where blocks may be laid. Code, a table's cell and a caption take the text as it is. */
export function acceptsBlocks(state: EditorState): boolean {
  const tr = state.tr;
  if (!state.selection.empty) tr.deleteSelection();
  const $from = tr.doc.resolve(tr.selection.from);
  const at = containerAt(tr.doc, $from.pos);
  return at?.node.type.name === 'block' && isTextContent($from.parent);
}

/** Lays what a text said on the page at the caret. `source` is the text as it was pasted, for the space around its words. Answers whether it did. */
export function pastePieces(state: EditorState, pieces: readonly Piece[], source: string, dispatch: (tr: Transaction) => void): boolean {
  if (pieces.length === 0 || !acceptsBlocks(state)) return false;
  const tr = state.tr;
  if (!state.selection.empty) tr.deleteSelection();
  const $from = tr.doc.resolve(tr.selection.from);
  const at = containerAt(tr.doc, $from.pos);
  if (at === null) return false;

  const only = pieces.length === 1 ? pieces[0] : undefined;
  if (only?.type === 'paragraph' && only.children.length === 0) {
    tr.insert(tr.selection.from, lineOf(state, only, source));
    dispatch(tr.scrollIntoView());
    return true;
  }

  const nodes = blocksOf(state, pieces);
  const first = at.node.child(0);
  const empty = at.node.childCount === 1 && isTextContent(first) && first.content.size === 0;
  const start = empty ? at.pos : at.pos + at.node.nodeSize;
  if (empty) tr.replaceWith(at.pos, at.pos + at.node.nodeSize, nodes);
  else tr.insert(start, nodes);
  const end = start + nodes.reduce((sum, node) => sum + node.nodeSize, 0);
  // A page that ends in a block with no text needs a line after it to write in.
  const last = nodes[nodes.length - 1];
  const after = tr.doc.resolve(end);
  if (last !== undefined && last.type.name !== 'block' && after.nodeAfter === null) {
    tr.insert(end, state.schema.nodeFromJSON({ type: 'block', attrs: { id: newId() }, content: [{ type: 'paragraph' }] }));
  }
  tr.setSelection(Selection.near(tr.doc.resolve(Math.min(end, tr.doc.content.size)), last?.type.name === 'leafBlock' ? 1 : -1));
  dispatch(tr.scrollIntoView());
  return true;
}

export interface PasteNotice { text: string; error: boolean }

/** Reads a text with the server's reader and lays it on the page where the caret is then, saying what the reader kept as words or left out. */
export async function readAndPaste(
  view: { state: EditorState; dispatch: (tr: Transaction) => void; isDestroyed: boolean },
  convert: Convert,
  report: (notice: PasteNotice | null) => void,
  format: 'markdown' | 'html',
  text: string,
  plain: string,
): Promise<boolean> {
  const slow = setTimeout(() => report({ text: i18n.t('documents.paste.reading'), error: false }), SLOW_READING_MS);
  try {
    const read = await convert(format, text);
    clearTimeout(slow);
    if (view.isDestroyed) return false;
    const done = read.blocks.length === 0 || pastePieces(view.state, piecesOf(read.blocks), plain, (tr) => view.dispatch(tr));
    report(read.warnings.length === 0 ? null : { text: read.warnings.map((warning) => readerWarningText(warning)).join(' · '), error: false });
    return done;
  } catch (error) {
    clearTimeout(slow);
    report({ text: error instanceof Error ? error.message : String(error), error: true });
    return false;
  }
}

export const PasteMarkdown = Extension.create<{ report: (notice: PasteNotice | null) => void; convert: Convert }>({
  name: 'orchPasteMarkdown',
  addOptions() { return { report: () => undefined, convert: () => Promise.resolve({ blocks: [], warnings: [] }) }; },
  addProseMirrorPlugins() {
    return [
      new Plugin({
        key: new PluginKey('orchPasteMarkdown'),
        props: {
          handlePaste: (view, event) => {
            const data = event.clipboardData;
            if (data === null || !view.editable) return false;
            const plain = data.getData('text/plain');
            const html = data.getData('text/html');
            // Code, a table's cell and a caption take what was pasted as it is, without asking anybody.
            if (!acceptsBlocks(view.state)) return false;
            let format: 'markdown' | 'html' | null = null;
            let text = '';
            if (html !== '' && !html.includes('data-pm-slice')) {
              format = 'html';
              text = html;
            } else if (plain !== '' && !/<(h[1-6]|ul|ol|li|table|blockquote|p|img)\b/i.test(html) && looksLikeMarkdown(plain)) {
              // Rich text that has structure of its own is pasted as itself; the rest is Markdown if it says so.
              format = 'markdown';
              text = plain;
            }
            if (format === null) return false;
            event.preventDefault();
            void readAndPaste(view, this.options.convert, this.options.report, format, text, plain);
            return true;
          },
        },
      }),
    ];
  },
});
