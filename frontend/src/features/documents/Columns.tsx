/** Columns side by side with shares that are dragged; under one another on a narrow screen.
 *
 * The line between two columns is dragged to change their shares. A column that is emptied goes, and
 * the last one left becomes ordinary blocks again. Below `md` the columns stand under one another and
 * the lines between them are not drawn.
 */
import type { AnyExtension } from '@tiptap/core';
import type { Node as PmNode } from '@tiptap/pm/model';
import { Plugin } from '@tiptap/pm/state';
import { Extension } from '@tiptap/core';

import { Column } from '@/features/documents/schema';

/** The least a column keeps of the two that share a line, as a part of both. */
const LEAST_SHARE = 0.15;

/** A column, with the line on its right edge that is dragged to change its share. */
export const ColumnView = Column.extend({
  addNodeView() {
    return ({ node, getPos, editor }) => {
      let current = node;
      const dom = document.createElement('div');
      dom.className = 'ob-column';
      dom.dataset.ob = 'column';
      const body = document.createElement('div');
      body.className = 'ob-column-body';
      dom.append(body);
      const sash = document.createElement('div');
      sash.className = 'ob-column-sash';
      sash.contentEditable = 'false';
      sash.setAttribute('role', 'separator');
      sash.setAttribute('aria-orientation', 'vertical');
      dom.append(sash);
      const show = (next: PmNode) => {
        dom.style.setProperty('--share', String(next.attrs.share ?? 1));
        // The last column has no line on its right.
        const here = getPos();
        const parent = here === undefined ? null : editor.state.doc.resolve(here).parent;
        const last = parent !== null && parent.lastChild === next;
        sash.hidden = last || !editor.isEditable;
      };
      show(node);

      sash.addEventListener('pointerdown', (event) => {
        const here = getPos();
        if (here === undefined || !editor.isEditable) return;
        event.preventDefault();
        sash.setPointerCapture(event.pointerId);
        sash.classList.add('is-dragging');
        const parent = editor.state.doc.resolve(here).parent;
        const row = dom.parentElement;
        if (row === null) return;
        // Every column takes the width it has now as its share, so that the shares agree with one another.
        const widths = [...row.children].filter((child) => child.classList.contains('ob-column')).map((child) => child.getBoundingClientRect().width);
        const at = [...row.children].indexOf(dom);
        const left = widths[at] ?? 1;
        const right = widths[at + 1] ?? 1;
        const startX = event.clientX;
        let moved = 0;
        const move = (next: PointerEvent) => {
          const pair = left + right;
          const delta = Math.max(-(left - pair * LEAST_SHARE), Math.min(right - pair * LEAST_SHARE, next.clientX - startX));
          moved = delta;
          dom.style.setProperty('--share', String(left + delta));
          (row.children[at + 1] as HTMLElement | undefined)?.style.setProperty('--share', String(right - delta));
        };
        const end = (finish: PointerEvent) => {
          sash.releasePointerCapture(finish.pointerId);
          sash.classList.remove('is-dragging');
          sash.removeEventListener('pointermove', move);
          sash.removeEventListener('pointerup', end);
          sash.removeEventListener('pointercancel', end);
          if (moved === 0) return;
          const tr = editor.state.tr;
          // The first column of the set: every share is set in one change.
          const first = editor.state.doc.resolve(here).start();
          parent.forEach((_child, offset, index) => {
            const share = Math.round(index === at ? left + moved : index === at + 1 ? right - moved : (widths[index] ?? 1));
            tr.setNodeAttribute(first + offset, 'share', share);
          });
          editor.view.dispatch(tr);
        };
        sash.addEventListener('pointermove', move);
        sash.addEventListener('pointerup', end);
        sash.addEventListener('pointercancel', end);
      });

      return {
        dom,
        contentDOM: body,
        update(next: PmNode) {
          if (next.type !== current.type) return false;
          current = next;
          show(next);
          return true;
        },
        ignoreMutation: (mutation: { target: Node }) => !body.contains(mutation.target) && mutation.target !== body,
        stopEvent: (event: Event) => event.target === sash,
      };
    };
  },
});

const isEmpty = (column: PmNode): boolean =>
  column.childCount === 1 && column.child(0).childCount === 1 && column.child(0).child(0).type.name === 'paragraph' && column.child(0).child(0).content.size === 0;

/**
 * A column that is emptied goes, and when one column is left it becomes ordinary blocks again. Only a
 * column that had something in it and has nothing now is meant: columns just made are all empty and
 * are about to be written in.
 */
export const KeepColumns = Extension.create({
  name: 'orchKeepColumns',
  addProseMirrorPlugins() {
    return [
      new Plugin({
        appendTransaction(transactions, before, state) {
          if (!transactions.some((one) => one.docChanged)) return null;
          const filled = new Set<string>();
          before.doc.descendants((node) => {
            if (node.type.name === 'column' && !isEmpty(node)) filled.add(String(node.attrs.id));
            return node.type.name !== 'paragraph';
          });
          if (filled.size === 0) return null;
          const found: { pos: number; node: PmNode }[] = [];
          state.doc.descendants((node, pos) => {
            if (node.type.name === 'columns') found.push({ pos, node });
            return node.type.name !== 'paragraph' && node.type.name !== 'table';
          });
          let tr = state.tr;
          let changed = false;
          // From the end, so that earlier positions stay right.
          for (const { pos, node } of found.reverse()) {
            const kept: PmNode[] = [];
            node.forEach((column) => {
              if (!(isEmpty(column) && filled.has(String(column.attrs.id)))) kept.push(column);
            });
            if (kept.length === node.childCount) continue;
            changed = true;
            if (kept.length >= 2) {
              tr = tr.replaceWith(pos, pos + node.nodeSize, node.type.create(node.attrs, kept));
            } else {
              const blocks: PmNode[] = [];
              kept[0]?.forEach((child) => blocks.push(child));
              tr = tr.replaceWith(pos, pos + node.nodeSize, blocks);
            }
          }
          return changed ? tr : null;
        },
      }),
    ];
  },
});

export const columnViews: Readonly<Record<string, AnyExtension>> = { column: ColumnView };
