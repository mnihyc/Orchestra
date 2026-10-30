/** Colours of the code in a document's code blocks, by the parser of each block's language as the code viewer has it: fetched the first time a block of that language is drawn, and not before; a language without one stays text.
 *
 * The colours are the code viewer's own: the same groups of tags, given the same tokens of the palette, so
 * that a function looks the same in a document and in a file. A block is parsed when its text changes; one
 * that did not change keeps what it was given. A very long block stays text, so that typing in it stays quick.
 */
import type { Language as CodeLanguage } from '@codemirror/language';
import { highlightTree, tagHighlighter, tags } from '@lezer/highlight';
import { Extension } from '@tiptap/core';
import type { Node as PmNode } from '@tiptap/pm/model';
import { Plugin, PluginKey } from '@tiptap/pm/state';
import { Decoration, DecorationSet, type EditorView } from '@tiptap/pm/view';

import { languageById } from '@/features/files/languages';

type Parser = CodeLanguage['parser'];

/** The most characters of one block that are coloured: a log pasted whole stays text, and typing in it stays quick. */
const COLOURED_MAX_CHARS = 20_000;

/** How many coloured blocks are remembered by their text, so that a change elsewhere in the page parses none of them again. */
const REMEMBERED_BLOCKS = 200;

/** The classes the colours hang on: the code viewer's groups of tags, under the names the stylesheet gives them. */
const highlighter = tagHighlighter([
  { tag: [tags.keyword, tags.controlKeyword, tags.moduleKeyword, tags.operatorKeyword, tags.definitionKeyword], class: 'syn-keyword' },
  { tag: [tags.string, tags.special(tags.string), tags.regexp, tags.character], class: 'syn-string' },
  { tag: [tags.number, tags.bool, tags.null, tags.atom, tags.unit], class: 'syn-number' },
  { tag: [tags.comment, tags.lineComment, tags.blockComment, tags.docComment], class: 'syn-comment' },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName), tags.macroName, tags.propertyName], class: 'syn-function' },
  { tag: [tags.typeName, tags.className, tags.namespace, tags.standard(tags.typeName), tags.labelName], class: 'syn-type' },
  { tag: [tags.tagName, tags.angleBracket], class: 'syn-tag' },
  { tag: [tags.attributeName, tags.meta, tags.processingInstruction, tags.annotation, tags.documentMeta], class: 'syn-meta' },
  { tag: [tags.operator, tags.punctuation, tags.separator, tags.bracket], class: 'syn-operator' },
  { tag: [tags.link, tags.url], class: 'syn-link' },
]);

/** The names a code block is often given for a language the code viewer knows by another. */
const ALIASES: Readonly<Record<string, string>> = {
  bash: 'shell',
  sh: 'shell',
  zsh: 'shell',
  c: 'cpp',
  'c++': 'cpp',
  js: 'javascript',
  ts: 'typescript',
  py: 'python',
  rs: 'rust',
  golang: 'go',
  yml: 'yaml',
  md: 'markdown',
  rb: 'ruby',
  kt: 'kotlin',
  cs: 'csharp',
  'c#': 'csharp',
  docker: 'dockerfile',
  ps1: 'powershell',
  proto: 'protobuf',
  patch: 'diff',
};

/** The parser of each language asked for: `null` where there is none, a promise while it is fetched. */
const parsers = new Map<string, Parser | null | Promise<void>>();

/** The editors that draw code blocks: each is told when a parser arrives. */
const views = new Set<EditorView>();

/** What each block was given, by its language and its text: offsets in its text, and the classes there. */
const remembered = new Map<string, readonly (readonly [number, number, string])[]>();

const key = new PluginKey<DecorationSet>('orchCodeColours');

/** The meta of the transaction that tells an editor a parser arrived. */
const ARRIVED = 'parser-arrived';

/** Fetches the parser of a language once, and tells every editor when it is there. */
function want(id: string): void {
  const language = languageById(ALIASES[id] ?? id);
  if (language === null) {
    parsers.set(id, null);
    return;
  }
  const fetching = (async () => {
    try {
      const [loaded, codemirror] = await Promise.all([language.load(), import('@codemirror/language')]);
      const parser = loaded instanceof codemirror.LanguageSupport ? loaded.language.parser : loaded instanceof codemirror.Language ? loaded.parser : null;
      parsers.set(id, parser);
    } catch {
      parsers.set(id, null);
    }
    for (const view of views) view.dispatch(view.state.tr.setMeta(key, ARRIVED).setMeta('addToHistory', false));
  })();
  if (parsers.get(id) === undefined) parsers.set(id, fetching);
}

/** The pieces of `text` that carry a colour, as the parser reads it. */
function spans(parser: Parser, text: string): readonly (readonly [number, number, string])[] {
  const found: [number, number, string][] = [];
  highlightTree(parser.parse(text), highlighter, (from, to, classes) => {
    found.push([from, to, classes]);
  });
  return found;
}

/** The colours of every code block of a page whose language has a parser that has arrived. */
function colour(doc: PmNode): DecorationSet {
  const decorations: Decoration[] = [];
  doc.descendants((node, pos) => {
    if (node.type.name !== 'codeBlock') return true;
    const id = String(node.attrs.language ?? '')
      .trim()
      .toLowerCase();
    const text = node.textContent;
    if (id === '' || text === '' || text.length > COLOURED_MAX_CHARS) return false;
    const parser = parsers.get(id);
    if (parser === undefined) {
      want(id);
      return false;
    }
    if (parser === null || parser instanceof Promise) return false;
    const memo = `${id}\u0000${text}`;
    let found = remembered.get(memo);
    if (found === undefined) {
      found = spans(parser, text);
      if (remembered.size >= REMEMBERED_BLOCKS) {
        const oldest = remembered.keys().next().value;
        if (oldest !== undefined) remembered.delete(oldest);
      }
      remembered.set(memo, found);
    }
    // The text of a code block begins right after the block's own opening.
    for (const [from, to, classes] of found) decorations.push(Decoration.inline(pos + 1 + from, pos + 1 + to, { class: classes }));
    return false;
  });
  return DecorationSet.create(doc, decorations);
}

/** Colours code blocks as the code viewer colours files. */
export const CodeColours = Extension.create({
  name: 'orchCodeColours',
  addProseMirrorPlugins() {
    return [
      new Plugin<DecorationSet>({
        key,
        state: {
          init: (_config, state) => colour(state.doc),
          apply: (tr, before, _old, state) => (tr.docChanged || tr.getMeta(key) === ARRIVED ? colour(state.doc) : before),
        },
        props: {
          decorations: (state) => key.getState(state) ?? null,
        },
        view: (view) => {
          views.add(view);
          return {
            destroy: () => {
              views.delete(view);
            },
          };
        },
      }),
    ];
  },
});
