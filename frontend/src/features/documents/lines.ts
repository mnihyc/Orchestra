/** Views of the lines that carry something beside their text: the box of a to-do, the arrow of a block that folds, the number a list begins at, the language of a code block.
 *
 * They are plain elements, not components: a page may hold thousands of lines, and these draw in a
 * time that does not depend on a framework. Each keeps the text where the editor needs it (the content
 * element) and puts what the reader can press beside it, outside the text.
 */
import type { AnyExtension } from '@tiptap/core';
import type { Node as PmNode } from '@tiptap/pm/model';

import { Bullet, CodeBlock, Heading, Numbered, Toggle, Todo } from '@/features/documents/schema';
import { i18n } from '@/i18n';
import { copyText } from '@/ui/clipboard';

/** The languages a code block offers. The name is what is stored; a language not in the list is kept and shown as it is. */
export const CODE_LANGUAGES: readonly string[] = [
  '',
  'bash',
  'c',
  'cpp',
  'css',
  'diff',
  'go',
  'html',
  'java',
  'javascript',
  'json',
  'markdown',
  'python',
  'rust',
  'sql',
  'toml',
  'typescript',
  'yaml',
];

/** What a block's look says of itself to the stylesheet. */
function showLook(dom: HTMLElement, node: PmNode): void {
  const set = (name: string, value: unknown) => {
    const text = typeof value === 'string' ? value : typeof value === 'number' || value === true ? String(value) : null;
    if (text === null) dom.removeAttribute(`data-${name}`);
    else dom.setAttribute(`data-${name}`, text);
  };
  set('color', node.attrs.color);
  set('background', node.attrs.background);
  set('align', node.attrs.align);
  set('tone', node.attrs.tone);
  set('checked', node.type.name === 'todo' ? node.attrs.checked === true : null);
  set('level', node.attrs.level);
  set('start', node.attrs.start);
  if (node.attrs.start !== null && node.attrs.start !== undefined) dom.style.setProperty('--start', String(node.attrs.start));
  else dom.style.removeProperty('--start');
  if (node.type.name === 'toggle' || (node.type.name === 'heading' && node.attrs.toggle === true)) set('folded', node.attrs.folded === true ? 'true' : 'false');
  else dom.removeAttribute('data-folded');
}

function element<K extends keyof HTMLElementTagNameMap>(tag: K, className: string): HTMLElementTagNameMap[K] {
  const made = document.createElement(tag);
  made.className = className;
  return made;
}

/** The sign of Copy, drawn as the one icon the act has everywhere (two sheets, one over the other). */
function copyIcon(): SVGSVGElement {
  const NS = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(NS, 'svg');
  for (const [name, value] of [
    ['viewBox', '0 0 24 24'],
    ['fill', 'none'],
    ['stroke', 'currentColor'],
    ['stroke-width', '2'],
    ['stroke-linecap', 'round'],
    ['stroke-linejoin', 'round'],
    ['aria-hidden', 'true'],
  ] as const) {
    svg.setAttribute(name, value);
  }
  const sheet = document.createElementNS(NS, 'rect');
  for (const [name, value] of [
    ['width', '14'],
    ['height', '14'],
    ['x', '8'],
    ['y', '8'],
    ['rx', '2'],
    ['ry', '2'],
  ] as const) {
    sheet.setAttribute(name, value);
  }
  const under = document.createElementNS(NS, 'path');
  under.setAttribute('d', 'M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2');
  svg.append(sheet, under);
  return svg;
}

/** A line that folds what is under it, or has a box before its text, as a row: the mark, then the text. */
function rowView(node: PmNode, mark: (node: PmNode) => HTMLElement | null, textTag: string, textClass: string) {
  let current = node;
  const dom = element('div', 'oc oc-row');
  dom.dataset.ot = node.type.name;
  const text = element(textTag as 'div', textClass);
  const place = (made: HTMLElement | null) => {
    if (made !== null) dom.prepend(made);
  };
  let markEl = mark(node);
  place(markEl);
  dom.append(text);
  showLook(dom, node);
  return {
    dom,
    contentDOM: text,
    update(next: PmNode) {
      if (next.type !== current.type || next.attrs.level !== current.attrs.level) return false;
      current = next;
      const wanted = mark(next);
      // A heading that did not fold and now does gets its arrow; the other way round it loses it.
      if ((wanted === null) !== (markEl === null)) {
        markEl?.remove();
        markEl = wanted;
        place(markEl);
      }
      showLook(dom, next);
      return true;
    },
    ignoreMutation: (mutation: { target: Node }) => mutation.target !== text && !text.contains(mutation.target),
    stopEvent: (event: Event) => markEl !== null && event.target instanceof Node && markEl.contains(event.target),
  };
}

const flip = (editor: { state: { tr: { setNodeAttribute: (pos: number, name: string, value: unknown) => unknown } }; view: { dispatch: (tr: never) => void }; isEditable: boolean }, pos: number | undefined, name: string, value: unknown) => {
  if (pos === undefined || (!editor.isEditable && name !== 'folded')) return;
  const tr = editor.state.tr.setNodeAttribute(pos, name, value);
  // Folding is the reader's own and nobody's to undo; a tick is a change like any other.
  if (name === 'folded') (tr as { setMeta: (key: string, value: unknown) => void }).setMeta('addToHistory', false);
  editor.view.dispatch(tr as never);
};

function foldMark(node: PmNode, onFlip: (folded: boolean) => void): HTMLElement | null {
  if (!(node.type.name === 'toggle' || (node.type.name === 'heading' && node.attrs.toggle === true))) return null;
  const button = element('button', 'oc-fold');
  button.type = 'button';
  button.contentEditable = 'false';
  button.tabIndex = -1;
  button.setAttribute('aria-label', i18n.t('documents.block.fold'));
  button.addEventListener('mousedown', (event) => event.preventDefault());
  button.addEventListener('click', () => onFlip(true));
  return button;
}

/** Folds what is under a toggle or a heading that folds; the arrow only turns it. */
export const ToggleView = Toggle.extend({
  addNodeView() {
    return ({ node, getPos, editor }) => {
      let at = node;
      const view = rowView(node, (current) => foldMark(current, () => flip(editor, getPos(), 'folded', at.attrs.folded !== true)), 'div', 'oc-text');
      return {
        ...view,
        update(next: PmNode) {
          at = next;
          return view.update(next);
        },
      };
    };
  },
});

export const HeadingView = Heading.extend({
  addNodeView() {
    return ({ node, getPos, editor }) => {
      let at = node;
      const view = rowView(
        node,
        (current) => foldMark(current, () => flip(editor, getPos(), 'folded', at.attrs.folded !== true)),
        `h${Math.min(6, Math.max(1, Number(node.attrs.level) || 1))}`,
        'oc-text',
      );
      return {
        ...view,
        update(next: PmNode) {
          at = next;
          return view.update(next);
        },
      };
    };
  },
});

/** The box before a to-do's text, which ticks it. */
export const TodoView = Todo.extend({
  addNodeView() {
    return ({ node, getPos, editor }) => {
      const view = rowView(
        node,
        (current) => {
          const wrap = element('span', 'oc-mark');
          const box = element('input', 'oc-check');
          box.type = 'checkbox';
          box.contentEditable = 'false';
          box.tabIndex = -1;
          box.checked = current.attrs.checked === true;
          box.setAttribute('aria-label', i18n.t('documents.block.doneNamed', { name: current.textContent }));
          box.addEventListener('change', () => {
            if (!editor.isEditable) {
              box.checked = !box.checked;
              return;
            }
            flip(editor, getPos(), 'checked', box.checked);
          });
          wrap.append(box);
          return wrap;
        },
        'div',
        'oc-text',
      );
      const box = view.dom.querySelector('input');
      return {
        ...view,
        update(next: PmNode) {
          if (box !== null) { box.checked = next.attrs.checked === true; box.setAttribute('aria-label', i18n.t('documents.block.doneNamed', { name: next.textContent })); }
          return view.update(next);
        },
      };
    };
  },
});

/** A numbered item that begins its list at a number of its own. */
export const NumberedView = Numbered.extend({
  addNodeView() {
    return ({ node }) => {
      const dom = element('div', 'oc');
      dom.dataset.ot = 'numbered';
      showLook(dom, node);
      return {
        dom,
        contentDOM: dom,
        update(next: PmNode) {
          if (next.type !== node.type) return false;
          showLook(dom, next);
          return true;
        },
      };
    };
  },
});

/** A code block with its language to choose and a switch for long lines. */
export const CodeView = CodeBlock.extend({
  addNodeView() {
    return ({ node, getPos, editor }) => {
      const dom = element('div', 'oc oc-code');
      dom.dataset.ot = 'codeBlock';
      const head = element('div', 'oc-code-head');
      head.contentEditable = 'false';
      const select = element('select', '');
      select.setAttribute('aria-label', i18n.t('documents.code.language'));
      const options = (language: string) => (CODE_LANGUAGES.includes(language) ? CODE_LANGUAGES : [...CODE_LANGUAGES, language]);
      const fill = (language: string) => {
        select.replaceChildren(
          ...options(language).map((one) => {
            const option = document.createElement('option');
            option.value = one;
            option.textContent = one === '' ? i18n.t('documents.code.plain') : one;
            return option;
          }),
        );
        select.value = language;
      };
      fill(String(node.attrs.language ?? ''));
      select.addEventListener('change', () => {
        if (!editor.isEditable) {
          select.value = String(node.attrs.language ?? '');
          return;
        }
        flip(editor, getPos(), 'language', select.value);
      });
      select.addEventListener('mousedown', (event) => event.stopPropagation());
      // Copy takes the whole code as it is written, for whoever reads the page.
      const copy = element('button', 'oc-code-copy');
      copy.type = 'button';
      copy.setAttribute('aria-label', i18n.t('ui.code.copy'));
      copy.title = i18n.t('ui.code.copy');
      copy.append(copyIcon());
      copy.addEventListener('mousedown', (event) => event.stopPropagation());
      copy.addEventListener('click', () => void copyText(current.textContent));
      head.append(select, copy);
      const body = element('pre', '');
      body.dataset.contentRole = 'source';
      body.spellcheck = false;
      dom.append(head, body);
      dom.dataset.wrap = node.attrs.wrap === true ? 'true' : 'false';
      let current = node;
      return {
        dom,
        contentDOM: body,
        update(next: PmNode) {
          if (next.type !== current.type) return false;
          if (next.attrs.language !== current.attrs.language) fill(String(next.attrs.language ?? ''));
          dom.dataset.wrap = next.attrs.wrap === true ? 'true' : 'false';
          current = next;
          return true;
        },
        ignoreMutation: (mutation: { target: Node }) => !body.contains(mutation.target) && mutation.target !== body,
        stopEvent: (event: Event) => event.target instanceof Node && head.contains(event.target),
      };
    };
  },
});

/** The plain look of the lines that need no view of their own is set from their attributes in the stylesheet; bullets and quotes keep the plain ones. */
export const lineViews: Readonly<Record<string, AnyExtension>> = {
  toggle: ToggleView,
  heading: HeadingView,
  todo: TodoView,
  numbered: NumberedView,
  codeBlock: CodeView,
  bullet: Bullet,
};
