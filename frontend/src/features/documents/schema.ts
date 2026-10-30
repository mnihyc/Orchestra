/** The twenty block types and their inline content, and the mapping between the editor's state and Orch's blocks.
 *
 * This is the one place that knows how a page of blocks looks to the editor: every block is a
 * container with a content node (the text, the formula, the table) and, for blocks that may have
 * them, a group of children, so that the editor's tree is the document's tree. The editor's state is
 * turned into drafts here and back, and nowhere else, so that the editor can be replaced without
 * touching a document.
 */
import { getSchema, mergeAttributes, Node, Mark, type AnyExtension } from '@tiptap/core';
import type { JSONContent, NodeViewRenderer } from '@tiptap/core';
import { Bold } from '@tiptap/extension-bold';
import { Code } from '@tiptap/extension-code';
import { HardBreak } from '@tiptap/extension-hard-break';
import { Italic } from '@tiptap/extension-italic';
import { Link } from '@tiptap/extension-link';
import { Strike } from '@tiptap/extension-strike';
import { TableCell, TableRow, Table } from '@tiptap/extension-table';
import { Text } from '@tiptap/extension-text';
import { Underline } from '@tiptap/extension-underline';
import { UniqueID } from '@tiptap/extension-unique-id';
import type { Node as PmNode, NodeType, Schema } from '@tiptap/pm/model';

import type { BlockType, Content, Draft, Props, Tree } from '@/features/documents/blocks';
import type { Inline, RefPart, TableRow as OrchRow } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { linkAllowed as safeWritingLink } from '@/features/people/links';
import { ReactNodeViewRenderer } from '@tiptap/react';
import { ReferenceView } from '@/features/people/ReferenceView';
import { MentionView } from '@/features/people/MentionView';

// ---------------------------------------------------------------- the kinds of block

/** How one block type is held by the editor. */
interface Kind {
  /** The content node. */
  node: string;
  /** The container it stands in: one that may have children, or one that may not. */
  container: 'block' | 'leafBlock';
  /** The props the type takes, which are the attributes of its content node. */
  props: readonly string[];
}

const LOOK = ['color', 'background', 'align'] as const;

/** The block types that are held as a container with a content node (columns and column are containers of their own). */
export const KINDS: Readonly<Record<Exclude<BlockType, 'columns' | 'column'>, Kind>> = {
  paragraph: { node: 'paragraph', container: 'block', props: LOOK },
  heading: { node: 'heading', container: 'block', props: ['level', 'toggle', ...LOOK] },
  bullet: { node: 'bullet', container: 'block', props: LOOK },
  numbered: { node: 'numbered', container: 'block', props: ['start', ...LOOK] },
  todo: { node: 'todo', container: 'block', props: ['checked', ...LOOK] },
  toggle: { node: 'toggle', container: 'block', props: LOOK },
  quote: { node: 'quote', container: 'block', props: LOOK },
  callout: { node: 'callout', container: 'block', props: ['tone', ...LOOK] },
  code: { node: 'codeBlock', container: 'leafBlock', props: ['language', 'wrap'] },
  math: { node: 'math', container: 'leafBlock', props: ['source'] },
  diagram: { node: 'diagram', container: 'leafBlock', props: ['source'] },
  divider: { node: 'divider', container: 'leafBlock', props: [] },
  page_break: { node: 'pageBreak', container: 'leafBlock', props: [] },
  image: { node: 'image', container: 'leafBlock', props: ['file_id', 'width', 'align', 'display'] },
  video: { node: 'video', container: 'leafBlock', props: ['file_id', 'url', 'width', 'display'] },
  audio: { node: 'audio', container: 'leafBlock', props: ['file_id', 'url', 'width', 'display'] },
  file: { node: 'file', container: 'leafBlock', props: ['file_id', 'display'] },
  table: { node: 'table', container: 'leafBlock', props: ['header_row', 'header_column', 'widths'] },
};

/** What an attribute is when its prop was left out. */
const ATTR_DEFAULTS: Readonly<Record<string, unknown>> = {
  level: 1,
  toggle: false,
  start: null,
  checked: false,
  tone: 'info',
  language: '',
  wrap: false,
  source: '',
  file_id: null,
  url: null,
  width: null,
  align: null,
  color: null,
  background: null,
  header_row: false,
  header_column: false,
  widths: null,
  share: null,
};

const TYPE_OF_NODE = new Map<string, BlockType>(Object.entries(KINDS).map(([type, kind]) => [kind.node, type as BlockType]));

/** The text types: their content node has inline content that is text. */
export const TEXT_NODES: readonly string[] = ['paragraph', 'heading', 'bullet', 'numbered', 'todo', 'toggle', 'quote', 'callout'];
/** The leaf types that carry a caption of inline content. */
export const CAPTIONED_NODES: readonly string[] = ['image', 'video', 'audio', 'file'];
/** The twelve colours of the palette that a block, a cell and a run of text may take. */
export const TINTS = 12;
/** The widest the columns of a table are drawn together, in pixels: the reading column. */
export const TABLE_WIDTH = 760;

function dashed(name: string): string {
  return name.replace(/_/g, '-');
}

/** An attribute that survives being copied as HTML: it is written as `data-…` when it is not its default, and read back. */
function attribute(name: string, kind: 'string' | 'number' | 'boolean' | 'json' = 'string') {
  const fallback = ATTR_DEFAULTS[name] ?? null;
  return {
    default: fallback,
    parseHTML: (element: HTMLElement): unknown => {
      const text = element.getAttribute(`data-${dashed(name)}`);
      if (text === null) return fallback;
      if (kind === 'number') return Number(text);
      if (kind === 'boolean') return text === 'true';
      if (kind === 'json') {
        try {
          return JSON.parse(text) as unknown;
        } catch {
          return fallback;
        }
      }
      return text;
    },
    renderHTML: (attributes: Record<string, unknown>): Record<string, string> => {
      const value = attributes[name];
      if (value === null || value === undefined || value === fallback) return {};
      const text = typeof value === 'object' ? JSON.stringify(value) : typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean' ? String(value) : null;
      return text === null ? {} : { [`data-${dashed(name)}`]: text };
    },
  };
}

const looks = () => ({ color: attribute('color', 'number'), background: attribute('background', 'number'), align: attribute('align') });

/** Whether a block that folds is folded: the reader's own, never sent. */
const folding = () => ({ folded: { default: false, rendered: false, parseHTML: () => false } });

// ---------------------------------------------------------------- containers

export const Doc = Node.create({ name: 'doc', topNode: true, content: 'blockItem+' });

function containerView(kind: string, classes: string): NodeViewRenderer {
  return ({ node, HTMLAttributes }) => {
    const dom = document.createElement('div');
    for (const [name, value] of Object.entries(mergeAttributes(HTMLAttributes, { class: classes, 'data-ob': kind }))) dom.setAttribute(name, String(value));
    return {
      dom,
      contentDOM: dom,
      update: (next) => next.sameMarkup(node),
      // A modal picker hides the background from assistive tools. Those attributes
      // change no content; reparsing them would destroy the reference card's node view.
      ignoreMutation: (mutation) => mutation.type === 'attributes' && mutation.target === dom
        && (mutation.attributeName === 'aria-hidden' || mutation.attributeName === 'data-aria-hidden'),
    };
  };
}

/** A block that may have children: its content node, then the group of its children. */
export const Block = Node.create({
  name: 'block',
  group: 'blockItem',
  content: 'blockContent blockGroup?',
  defining: true,
  addNodeView: () => containerView('block', 'ob'),
  parseHTML: () => [{ tag: 'div[data-ob="block"]' }],
  renderHTML: ({ HTMLAttributes }) => ['div', mergeAttributes(HTMLAttributes, { class: 'ob', 'data-ob': 'block' }), 0],
});

/** A block that may not: its content node alone. */
export const LeafBlock = Node.create({
  name: 'leafBlock',
  group: 'blockItem',
  content: 'leafContent',
  defining: true,
  addNodeView: () => containerView('leaf', 'ob'),
  parseHTML: () => [{ tag: 'div[data-ob="leaf"]' }],
  renderHTML: ({ HTMLAttributes }) => ['div', mergeAttributes(HTMLAttributes, { class: 'ob', 'data-ob': 'leaf' }), 0],
});

/** The children of a block, which stand one indent in. */
export const BlockGroup = Node.create({
  name: 'blockGroup',
  content: 'blockItem+',
  addNodeView: () => containerView('group', 'ob-children'),
  parseHTML: () => [{ tag: 'div[data-ob="group"]' }],
  renderHTML: ({ HTMLAttributes }) => ['div', mergeAttributes(HTMLAttributes, { class: 'ob-children', 'data-ob': 'group' }), 0],
});

/** Two to six columns side by side. */
export const Columns = Node.create({
  name: 'columns',
  group: 'blockItem',
  content: 'column{2,6}',
  defining: true,
  isolating: true,
  addNodeView: () => containerView('columns', 'ob ob-columns'),
  parseHTML: () => [{ tag: 'div[data-ob="columns"]' }],
  renderHTML: ({ HTMLAttributes }) => ['div', mergeAttributes(HTMLAttributes, { class: 'ob ob-columns', 'data-ob': 'columns' }), 0],
});

/** One column: any blocks but another set of columns. */
export const Column = Node.create({
  name: 'column',
  content: '(block | leafBlock)+',
  defining: true,
  isolating: true,
  addAttributes: () => ({ share: attribute('share', 'number') }),
  parseHTML: () => [{ tag: 'div[data-ob="column"]' }],
  renderHTML: ({ HTMLAttributes }) => ['div', mergeAttributes(HTMLAttributes, { class: 'ob-column', 'data-ob': 'column' }), 0],
});

// ---------------------------------------------------------------- content nodes

function textNode(name: string, extra: () => Record<string, object> = () => ({})) {
  return Node.create({
    name,
    group: 'blockContent',
    content: 'inline*',
    defining: true,
    addAttributes: () => ({ ...looks(), ...extra() }),
    parseHTML: () => [{ tag: `div[data-ot="${name}"]` }],
    renderHTML: ({ HTMLAttributes }) => ['div', mergeAttributes(HTMLAttributes, { class: 'oc', 'data-ot': name }), 0],
  });
}

export const Paragraph = Node.create({
  name: 'paragraph',
  group: 'blockContent',
  content: 'inline*',
  defining: true,
  addAttributes: () => looks(),
  parseHTML: () => [{ tag: 'p' }],
  renderHTML: ({ HTMLAttributes }) => ['p', mergeAttributes(HTMLAttributes, { class: 'oc', 'data-ot': 'paragraph' }), 0],
});

export const Heading = Node.create({
  name: 'heading',
  group: 'blockContent',
  content: 'inline*',
  defining: true,
  addAttributes: () => ({ level: attribute('level', 'number'), toggle: attribute('toggle', 'boolean'), ...looks(), ...folding() }),
  parseHTML: () => [1, 2, 3, 4, 5, 6].map((level) => ({ tag: `h${level}`, attrs: { level } })),
  renderHTML: ({ node, HTMLAttributes }) => [
    `h${Math.min(6, Math.max(1, Number(node.attrs.level) || 1))}`,
    mergeAttributes(HTMLAttributes, { class: 'oc', 'data-ot': 'heading' }),
    0,
  ],
});

export const Bullet = textNode('bullet');
export const Numbered = textNode('numbered', () => ({ start: attribute('start', 'number') }));
export const Todo = textNode('todo', () => ({ checked: attribute('checked', 'boolean') }));
export const Toggle = textNode('toggle', folding);
export const Quote = textNode('quote');
export const Callout = textNode('callout', () => ({ tone: attribute('tone') }));

/** Code: one unmarked text, with a language. */
export const CodeBlock = Node.create({
  name: 'codeBlock',
  group: 'leafContent',
  content: 'text*',
  marks: '',
  code: true,
  defining: true,
  addAttributes: () => ({ language: attribute('language'), wrap: attribute('wrap', 'boolean') }),
  parseHTML: () => [{ tag: 'pre', preserveWhitespace: 'full' }],
  renderHTML: ({ HTMLAttributes }) => ['pre', mergeAttributes(HTMLAttributes, { class: 'oc', 'data-ot': 'codeBlock' }), ['code', 0]],
});

function sourceNode(name: string) {
  return Node.create({
    name,
    group: 'leafContent',
    atom: true,
    // A click selects the whole block, which is what Delete then takes away.
    selectable: false,
    addAttributes: () => ({ source: attribute('source') }),
    parseHTML: () => [{ tag: `div[data-ot="${name}"]` }],
    renderHTML: ({ node, HTMLAttributes }) => ['div', mergeAttributes(HTMLAttributes, { class: 'oc', 'data-ot': name }), String(node.attrs.source ?? '')],
  });
}
export const MathBlock = sourceNode('math');
export const DiagramBlock = sourceNode('diagram');

function bareNode(name: string) {
  return Node.create({
    name,
    group: 'leafContent',
    atom: true,
    selectable: false,
    parseHTML: () => [{ tag: `div[data-ot="${name}"]` }],
    renderHTML: ({ HTMLAttributes }) => [
      'div',
      mergeAttributes(HTMLAttributes, { class: 'oc', 'data-ot': name, ...(name === 'pageBreak' ? { 'data-label': i18n.t('documents.blocks.page_break') } : {}) }),
      ['hr'],
    ],
  });
}
export const Divider = bareNode('divider');
export const PageBreak = bareNode('pageBreak');

/** A block of media or a file: what it shows is drawn by its view; its content is the caption. */
function mediaNode(name: string, props: readonly string[]) {
  return Node.create({
    name,
    group: 'leafContent',
    content: 'inline*',
    defining: true,
    isolating: true,
    addAttributes: () => Object.fromEntries(props.map((prop) => [prop, attribute(prop, prop === 'width' ? 'number' : 'string')])),
    parseHTML: () => [{ tag: `figure[data-ot="${name}"]` }],
    renderHTML: ({ HTMLAttributes }) => [
      'figure',
      mergeAttributes(HTMLAttributes, { class: 'oc', 'data-ot': name }),
      ['div', { class: 'oc-media', contenteditable: 'false' }],
      ['figcaption', 0],
    ],
  });
}
export const ImageBlock = mediaNode('image', KINDS.image.props);
export const VideoBlock = mediaNode('video', KINDS.video.props);
export const AudioBlock = mediaNode('audio', KINDS.audio.props);
export const FileBlock = mediaNode('file', KINDS.file.props);

/** A table whose cells hold inline content alone. */
export const OrchTable = Table.extend({
  group: 'leafContent',
  addAttributes() {
    return {
      header_row: attribute('header_row', 'boolean'),
      header_column: attribute('header_column', 'boolean'),
      widths: attribute('widths', 'json'),
    };
  },
});
export const OrchTableRow = TableRow.extend({ content: 'tableCell+' });
export const OrchCell = TableCell.extend({
  content: 'inline*',
  addAttributes() {
    return {
      colspan: { default: 1 },
      rowspan: { default: 1 },
      colwidth: {
        default: null,
        parseHTML: (element: HTMLElement) => {
          const widths = element.getAttribute('data-colwidth');
          return widths === null ? null : widths.split(',').map(Number);
        },
      },
      ...looks(),
    };
  },
});

// ---------------------------------------------------------------- inline

/** A person named in a line. */
export const Mention = Node.create({
  name: 'mention',
  group: 'inline',
  inline: true,
  atom: true,
  selectable: true,
  addNodeView: () => ReactNodeViewRenderer(MentionView),
  addAttributes: () => ({ user_id: { default: null }, name: { default: null }, username: { default: null }, user: { default: null, renderHTML: () => ({}) }, can_view: { default: null, renderHTML: () => ({}) }, prompt: { default: false, renderHTML: () => ({}) } }),
  parseHTML: () => [{ tag: 'span[data-mention]' }],
  renderHTML: ({ node, HTMLAttributes }) => [
    'span',
    mergeAttributes(HTMLAttributes, { class: 'oi oi-mention', 'data-mention': String(node.attrs.user_id ?? '') }),
    `@${String(node.attrs.name ?? node.attrs.username ?? '')}`,
  ],
});

/** A chip that names a node, a document or a chat of Orch, and may name one of its parts. */
export const Reference = Node.create({
  name: 'ref',
  group: 'inline',
  inline: true,
  atom: true,
  selectable: true,
  addNodeView: () => ReactNodeViewRenderer(ReferenceView),
  addAttributes: () => ({ kind: { default: 'node' }, id: { default: null }, path: attribute('path'), part: attribute('part', 'json'), label: attribute('label'), display: attribute('display'), name: { default: null } }),
  parseHTML: () => [{ tag: 'span[data-ref]' }],
  renderHTML: ({ node, HTMLAttributes }) => [
    'span',
    mergeAttributes(HTMLAttributes, { class: 'oi oi-ref', 'data-ref': String(node.attrs.id ?? ''), 'data-kind': String(node.attrs.kind) }),
    String(node.attrs.name ?? ''),
  ],
});

/** A formula in a line. */
export const InlineMath = Node.create({
  name: 'inlineMath',
  group: 'inline',
  inline: true,
  atom: true,
  selectable: true,
  addAttributes: () => ({ source: { default: '' } }),
  parseHTML: () => [{ tag: 'span[data-inline-math]' }],
  renderHTML: ({ node, HTMLAttributes }) => [
    'span',
    mergeAttributes(HTMLAttributes, { class: 'oi oi-math', 'data-inline-math': '' }),
    String(node.attrs.source ?? ''),
  ],
});

function tintMark(name: string, tag: string, data: string) {
  return Mark.create({
    name,
    addAttributes: () => ({ tint: { default: 0, parseHTML: (element: HTMLElement) => Number(element.getAttribute(data) ?? 0), renderHTML: () => ({}) } }),
    parseHTML: () => [{ tag: `${tag}[${data}]` }],
    renderHTML: ({ mark }) => [tag, { [data]: String(mark.attrs.tint) }, 0],
  });
}
/** The colour of some text, one of the twelve of the palette. */
export const TextColor = tintMark('textColor', 'span', 'data-text-tint');
/** The colour behind some text. */
export const Highlight = tintMark('highlight', 'mark', 'data-bg-tint');

// ---------------------------------------------------------------- the schema

/** Where a link may go: a path of this system, or an address that opens a page, a mail or nothing else. */
export function linkAllowed(address: string): boolean {
  return safeWritingLink(address);
}

/** Everything the page of blocks is made of; `views` puts a version with a view of its own in the place of the plain one of that name. */
export function schemaExtensions(views: Readonly<Record<string, AnyExtension>> = {}): AnyExtension[] {
  const plain: AnyExtension[] = [
    Doc,
    Text,
    Block,
    LeafBlock,
    BlockGroup,
    Columns,
    Column,
    Paragraph,
    Heading,
    Bullet,
    Numbered,
    Todo,
    Toggle,
    Quote,
    Callout,
    CodeBlock,
    MathBlock,
    DiagramBlock,
    Divider,
    PageBreak,
    ImageBlock,
    VideoBlock,
    AudioBlock,
    FileBlock,
    OrchTable,
    OrchTableRow,
    OrchCell,
    HardBreak,
    Mention,
    Reference,
    InlineMath,
    Bold,
    Italic,
    Underline,
    Strike,
    // A run may be code and bold and a link at once, as the blocks allow.
    Code.extend({ excludes: '' }),
    Link.configure({ openOnClick: false, autolink: true, linkOnPaste: true, isAllowedUri: (url) => linkAllowed(url) }),
    TextColor,
    Highlight,
    // Every container has an identifier from the moment it exists, the same on every screen, and a copy of one gets its own.
    UniqueID.configure({ types: ['block', 'leafBlock', 'columns', 'column'], attributeName: 'id', generateID: () => newId() }),
  ];
  return plain.map((extension) => views[extension.name] ?? extension);
}

/** A type of node of the schema by its name. The schema is Orch's own and has every one of them. */
export function nodeOf(schema: Schema, name: string): NodeType {
  const type = schema.nodes[name];
  if (type === undefined) throw new Error(`the schema has no node called ${name}`);
  return type;
}

let schemaOnce: Schema | null = null;

/** The schema of a page of blocks. */
export function orchSchema(): Schema {
  schemaOnce ??= getSchema(schemaExtensions());
  return schemaOnce;
}

// ---------------------------------------------------------------- inline content, both ways

function marksOf(run: Extract<Inline, { type: 'text' }>): { type: string; attrs?: Record<string, unknown> }[] {
  const marks: { type: string; attrs?: Record<string, unknown> }[] = (run.marks ?? []).map((name) => ({ type: name }));
  if (typeof run.href === 'string' && run.href !== '') marks.push({ type: 'link', attrs: { href: run.href } });
  if (typeof run.color === 'number') marks.push({ type: 'textColor', attrs: { tint: run.color } });
  if (typeof run.background === 'number') marks.push({ type: 'highlight', attrs: { tint: run.background } });
  return marks;
}

/** Inline content as the editor's nodes. */
export function inlinesToJson(runs: readonly Inline[]): JSONContent[] {
  const out: JSONContent[] = [];
  for (const run of runs) {
    switch (run.type) {
      case 'text': {
        if (run.text === '') break;
        const marks = marksOf(run);
        out.push({ type: 'text', text: run.text, ...(marks.length > 0 ? { marks } : {}) });
        break;
      }
      case 'mention':
        out.push({ type: 'mention', attrs: { user_id: run.user_id, name: run.user?.display_name ?? null, username: run.user?.username ?? null, user: run.user ?? null } });
        break;
      case 'ref':
        out.push({ type: 'ref', attrs: { kind: run.kind, id: run.id, path: run.path ?? null, part: run.part ?? null, name: run.name ?? null, label: run.label ?? null, display: run.display ?? null } });
        break;
      case 'math':
        out.push({ type: 'inlineMath', attrs: { source: run.source } });
        break;
      default:
        out.push({ type: 'hardBreak' });
    }
  }
  return out;
}

/** The inline content of a node of the editor. */
export function inlinesFromNode(node: PmNode): Inline[] {
  const out: Inline[] = [];
  node.forEach((child) => {
    if (child.isText) {
      const run: Extract<Inline, { type: 'text' }> = { type: 'text', text: child.text ?? '' };
      const marks: NonNullable<typeof run.marks> = [];
      for (const mark of child.marks) {
        const name = mark.type.name;
        if (name === 'bold' || name === 'italic' || name === 'underline' || name === 'strike' || name === 'code') marks.push(name);
        else if (name === 'link') run.href = String(mark.attrs.href ?? '');
        else if (name === 'textColor') run.color = Number(mark.attrs.tint);
        else if (name === 'highlight') run.background = Number(mark.attrs.tint);
      }
      if (marks.length > 0) run.marks = marks;
      out.push(run);
      return;
    }
    switch (child.type.name) {
      case 'mention':
        out.push({ type: 'mention', user_id: String(child.attrs.user_id ?? '') });
        break;
      case 'ref':
        out.push({
          type: 'ref',
          kind: child.attrs.kind === 'document' ? 'document' : child.attrs.kind === 'chat' ? 'chat' : child.attrs.kind === 'file' ? 'file' : child.attrs.kind === 'environment_file' ? 'environment_file' : 'node',
          id: String(child.attrs.id ?? ''),
          ...(typeof child.attrs.label === 'string' ? { label: child.attrs.label } : typeof child.attrs.name === 'string' ? { label: child.attrs.name } : {}),
          ...(child.attrs.display === 'small' || child.attrs.display === 'medium' || child.attrs.display === 'large' ? { display: child.attrs.display as 'small' | 'medium' | 'large' } : {}),
          ...(typeof child.attrs.path === 'string' ? { path: child.attrs.path } : {}),
          // A part an agent named stays with the chip through a person's writing around it.
          ...(child.attrs.part === null || child.attrs.part === undefined ? {} : { part: child.attrs.part as RefPart }),
        });
        break;
      case 'inlineMath':
        out.push({ type: 'math', source: String(child.attrs.source ?? '') });
        break;
      default:
        out.push({ type: 'break' });
    }
  });
  return out;
}

// ---------------------------------------------------------------- tables, both ways

/**
 * The table as the editor has it. The shares of the columns become widths in pixels across the
 * reading column, since that is what dragging the edge of a column changes.
 */
function tableToJson(props: Props, rows: readonly OrchRow[]): JSONContent[] {
  const shares = Array.isArray(props.widths) ? (props.widths as number[]) : null;
  const total = shares === null ? 0 : shares.reduce((sum, share) => sum + share, 0);
  const widthOf = (column: number): number | null => (shares === null || total <= 0 ? null : Math.round(((shares[column] ?? 0) / total) * TABLE_WIDTH));
  // Cells that a span from above covers are left out of their row, so each cell's column is found by walking the grid.
  const covered = new Map<number, number>();
  return rows.map((row) => {
    let column = 0;
    const cells = row.cells.map((cell) => {
      while ((covered.get(column) ?? 0) > 0) column += 1;
      const colspan = cell.colspan ?? 1;
      const rowspan = cell.rowspan ?? 1;
      const widths = Array.from({ length: colspan }, (_, index) => widthOf(column + index));
      const json: JSONContent = {
        type: 'tableCell',
        attrs: {
          colspan,
          rowspan,
          colwidth: widths.every((width) => width !== null) ? widths : null,
          color: cell.color ?? null,
          background: cell.background ?? null,
        },
        content: inlinesToJson(cell.content),
      };
      if (rowspan > 1) for (let index = 0; index < colspan; index += 1) covered.set(column + index, rowspan);
      column += colspan;
      return json;
    });
    // One row has passed: what a span from above still covers is a row less.
    for (const [key, left] of covered) if (left > 0) covered.set(key, left - 1);
    return { type: 'tableRow', content: cells };
  });
}

function tableFromNode(node: PmNode): OrchRow[] {
  const rows: OrchRow[] = [];
  node.forEach((row) => {
    const cells: OrchRow['cells'] = [];
    row.forEach((cell) => {
      const out: OrchRow['cells'][number] = { content: inlinesFromNode(cell) };
      if (Number(cell.attrs.colspan) > 1) out.colspan = Number(cell.attrs.colspan);
      if (Number(cell.attrs.rowspan) > 1) out.rowspan = Number(cell.attrs.rowspan);
      if (typeof cell.attrs.color === 'number') out.color = cell.attrs.color;
      if (typeof cell.attrs.background === 'number') out.background = cell.attrs.background;
      cells.push(out);
    });
    rows.push({ cells });
  });
  return rows;
}

/** The widths of a table's columns as shares, from the widths its first row's cells were dragged to; nothing when none was. */
function widthsOfTable(node: PmNode): number[] | null {
  const first = node.firstChild;
  if (first === null) return null;
  const widths: (number | null)[] = [];
  first.forEach((cell) => {
    const span = Number(cell.attrs.colspan) || 1;
    const dragged = cell.attrs.colwidth as number[] | null;
    for (let index = 0; index < span; index += 1) widths.push(dragged?.[index] ?? null);
  });
  if (widths.every((width) => width === null)) return null;
  const known = widths.filter((width): width is number => width !== null);
  const fill = known.reduce((sum, width) => sum + width, 0) / known.length;
  return widths.map((width) => Math.max(1, Math.round(width ?? fill)));
}

// ---------------------------------------------------------------- blocks to the editor

/** The attributes of a content node from the props of a block. */
export function attrsFromProps(type: Exclude<BlockType, 'columns' | 'column'>, props: Props): Record<string, unknown> {
  const attrs: Record<string, unknown> = {};
  for (const key of KINDS[type].props) attrs[key] = props[key] ?? ATTR_DEFAULTS[key] ?? null;
  return attrs;
}

/** The props of a block from the attributes of its content node. */
export function propsFromAttrs(type: Exclude<BlockType, 'columns' | 'column'>, attrs: Record<string, unknown>): Props {
  const props: Props = {};
  for (const key of KINDS[type].props) {
    const value = attrs[key];
    if (value !== null && value !== undefined) props[key] = value;
  }
  return props;
}

function contentJson(type: Exclude<BlockType, 'columns' | 'column'>, props: Props, content: Content): JSONContent {
  const kind = KINDS[type];
  const json: JSONContent = { type: kind.node, attrs: attrsFromProps(type, props) };
  switch (type) {
    case 'code': {
      const text = (content as Inline[]).map((run) => (run.type === 'text' ? run.text : '')).join('');
      if (text !== '') json.content = [{ type: 'text', text }];
      break;
    }
    case 'math':
    case 'diagram':
    case 'divider':
    case 'page_break':
      break;
    case 'table':
      json.content = tableToJson(props, content as OrchRow[]);
      break;
    default: {
      const inline = inlinesToJson(content as Inline[]);
      if (inline.length > 0) json.content = inline;
    }
  }
  return json;
}

/** The editor's JSON for one block, without what is under it. */
export function blockToJson(draft: Pick<Draft, 'id' | 'type' | 'props' | 'content'>): JSONContent {
  if (draft.type === 'columns') return { type: 'columns', attrs: { id: draft.id } };
  if (draft.type === 'column') return { type: 'column', attrs: { id: draft.id, share: (draft.props.share as number | undefined) ?? null } };
  const kind = KINDS[draft.type];
  return { type: kind.container, attrs: { id: draft.id }, content: [contentJson(draft.type, draft.props, draft.content)] };
}

/** The editor's JSON for a tree of blocks. A tree with nothing in it is one empty paragraph. */
export function treeToJson(tree: Tree): JSONContent {
  const build = (parent: string | null): JSONContent[] =>
    (tree.children.get(parent) ?? []).flatMap((id) => {
      const row = tree.rows.get(id);
      if (row === undefined) return [];
      const json = blockToJson({ id, type: row.type, props: row.props, content: row.content });
      const below = build(id);
      if (below.length === 0) return [json];
      if (row.type === 'columns') return [{ ...json, content: below }];
      if (row.type === 'column') return [{ ...json, content: below }];
      return [{ ...json, content: [...(json.content ?? []), { type: 'blockGroup', content: below }] }];
    });
  const items = build(null);
  return {
    type: 'doc',
    content: items.length > 0 ? items : [{ type: 'block', attrs: { id: newId() }, content: [{ type: 'paragraph' }] }],
  };
}

/** A fresh identifier for a block: one nobody used. */
export function newId(): string {
  return crypto.randomUUID();
}

// ---------------------------------------------------------------- the editor to blocks

/** What a content node says as a block: kept for as long as the node is the same one, so that a page is turned into drafts in a time that follows what changed. */
const said = new WeakMap<PmNode, Pick<Draft, 'type' | 'props' | 'content'>>();

/** The draft of one container of the editor, without what is under it. */
export function draftOf(container: PmNode, parentId: string | null): Draft {
  const id = String(container.attrs.id ?? '');
  if (container.type.name === 'columns') return { id, parentId, type: 'columns', props: {}, content: [] };
  if (container.type.name === 'column') {
    const share = container.attrs.share as number | null;
    return { id, parentId, type: 'column', props: share === null ? {} : { share }, content: [] };
  }
  const content = container.child(0);
  let body = said.get(content);
  if (body === undefined) {
    const type = TYPE_OF_NODE.get(content.type.name) ?? 'paragraph';
    body = { type, props: propsOf(type, content), content: contentOf(type, content) };
    said.set(content, body);
  }
  return { id, parentId, ...body };
}

function propsOf(type: BlockType, content: PmNode): Props {
  if (type === 'columns' || type === 'column') return {};
  const props = propsFromAttrs(type, content.attrs);
  if (type === 'table') {
    const widths = widthsOfTable(content);
    if (widths === null) delete props.widths;
    else props.widths = widths;
  }
  return props;
}

function contentOf(type: BlockType, content: PmNode): Content {
  switch (type) {
    case 'code':
      return content.textContent === '' ? [] : [{ type: 'text', text: content.textContent }];
    case 'math':
    case 'diagram':
    case 'divider':
    case 'page_break':
    case 'columns':
    case 'column':
      return [];
    case 'table':
      return tableFromNode(content);
    default:
      return inlinesFromNode(content);
  }
}

/** The blocks of the editor's document as drafts, each before what is under it. */
export function docToDrafts(doc: PmNode): Draft[] {
  const out: Draft[] = [];
  const walk = (parent: PmNode, parentId: string | null) => {
    parent.forEach((item) => {
      const draft = draftOf(item, parentId);
      out.push(draft);
      switch (item.type.name) {
        case 'block':
          if (item.childCount > 1) walk(item.child(1), draft.id);
          break;
        case 'columns':
        case 'column':
          walk(item, draft.id);
          break;
        default:
      }
    });
  };
  walk(doc, null);
  return out;
}

/** The new block that a type starts as, with the props the server needs of it. */
export function startingDraft(type: Exclude<BlockType, 'columns' | 'column'>, parentId: string | null = null): Draft {
  const props: Props = {};
  if (type === 'heading') props.level = 1;
  if (type === 'todo') props.checked = false;
  if (type === 'callout') props.tone = 'info';
  if (type === 'code') props.language = '';
  if (type === 'math' || type === 'diagram') props.source = '';
  if (type === 'table') props.header_row = false;
  return { id: newId(), parentId, type, props, content: [] };
}

/** A table of the given size, as rows of empty cells. */
export function emptyTable(rows: number, columns: number): OrchRow[] {
  return Array.from({ length: rows }, () => ({ cells: Array.from({ length: columns }, () => ({ content: [] })) }));
}
