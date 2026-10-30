/** Formulas and diagrams: the source in a small editor under the drawing, which follows as it is typed; loaded only on a page that has one.
 *
 * A formula is LaTeX drawn as mathematics, a diagram is Mermaid drawn as a picture. Neither library is
 * part of the page until a page has a formula or a diagram. A source that cannot be drawn keeps the last
 * drawing and says what is wrong. A diagram is shown as an image made of the drawing, so that the
 * styles inside it are its own and nothing in it can run.
 */
import type { AnyExtension } from '@tiptap/core';
import type Katex from 'katex';
import type Mermaid from 'mermaid';
import { NodeViewWrapper, ReactNodeViewRenderer, type NodeViewProps } from '@tiptap/react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { DiagramBlock, InlineMath, MathBlock } from '@/features/documents/schema';
import { cx } from '@/ui/cx';
import { StateLine } from '@/ui/StateLine';

/** How long a source rests after a key before it is drawn again. */
const DRAW_AFTER_MS = 250;

interface Drawing {
  /** What was drawn last: HTML for a formula, a picture's address for a diagram. */
  shown: string | null;
  /** How wide a diagram is drawn, in pixels: the width its own drawing says, so that a small one is not made to fill the page. */
  width: number | null;
  /** What is wrong with the source as it stands. */
  wrong: string | null;
}

/** The theme the page is in, so that a diagram is drawn for it. */
function useDark(): boolean {
  const [dark, setDark] = useState(() => isDark());
  useEffect(() => {
    const read = () => setDark(isDark());
    const watcher = new MutationObserver(read);
    watcher.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    media.addEventListener('change', read);
    return () => {
      watcher.disconnect();
      media.removeEventListener('change', read);
    };
  }, []);
  return dark;
}

function isDark(): boolean {
  const chosen = document.documentElement.dataset.theme;
  return chosen === 'dark' || (chosen !== 'light' && window.matchMedia('(prefers-color-scheme: dark)').matches);
}

let katexLoaded: Promise<typeof Katex> | null = null;
/** The formula library, fetched from this application's own files the first time a page has a formula. */
function loadKatex(): Promise<typeof Katex> {
  katexLoaded ??= (async () => {
    await import('katex/dist/katex.min.css');
    return (await import('katex')).default;
  })();
  return katexLoaded;
}

let mermaidLoaded: Promise<typeof Mermaid> | null = null;
function loadMermaid(): Promise<typeof Mermaid> {
  mermaidLoaded ??= import('mermaid').then((loaded) => loaded.default);
  return mermaidLoaded;
}

/** Draws a source after the person stops typing; keeps the last good drawing while the source is wrong. */
function useDrawing(kind: 'formula' | 'inline' | 'diagram', source: string): Drawing {
  const dark = useDark();
  const empty = source.trim() === '';
  const [drawing, setDrawing] = useState<Drawing>({ shown: null, width: null, wrong: null });
  const [wasEmpty, setWasEmpty] = useState(empty);
  // Clearing the source also forgets the last good drawing; a later invalid source must not revive it.
  if (empty !== wasEmpty) { setWasEmpty(empty); if (empty) setDrawing({ shown: null, width: null, wrong: null }); }
  const shownUrl = useRef<string | null>(null);
  useEffect(() => {
    if (source.trim() === '') {
      if (shownUrl.current !== null) { URL.revokeObjectURL(shownUrl.current); shownUrl.current = null; }
      return;
    }
    let alive = true;
    const timer = setTimeout(() => {
      void (async () => {
        try {
          if (kind === 'diagram') {
            const mermaid = await loadMermaid();
            mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme: dark ? 'dark' : 'default', fontFamily: 'inherit' });
            const id = `orch-diagram-${Math.random().toString(36).slice(2)}`;
            const { svg } = await mermaid.render(id, source);
            if (!alive) return;
            const url = URL.createObjectURL(new Blob([svg], { type: 'image/svg+xml' }));
            if (shownUrl.current !== null) URL.revokeObjectURL(shownUrl.current);
            shownUrl.current = url;
            const box = /viewBox="[-\d.]+[\s,]+[-\d.]+[\s,]+([\d.]+)[\s,]+[\d.]+"/.exec(svg);
            setDrawing({ shown: url, width: box?.[1] === undefined ? null : Math.ceil(Number(box[1])), wrong: null });
          } else {
            const katex = await loadKatex();
            const html = katex.renderToString(source, { displayMode: kind === 'formula', throwOnError: true, trust: false, output: 'htmlAndMathml' });
            if (alive) setDrawing({ shown: html, width: null, wrong: null });
          }
        } catch (error) {
          // The last drawing stays, and what is wrong with the source is said beside it.
          const reason = error instanceof Error ? error.message.split('\n')[0]?.trim() ?? '' : '';
          if (alive) setDrawing((last) => ({ shown: last.shown, width: last.width, wrong: /^[\w.:-]*$/u.test(reason) ? '' : reason }));
        }
      })();
    }, DRAW_AFTER_MS);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [kind, source, dark]);
  useEffect(
    () => () => {
      if (shownUrl.current !== null) URL.revokeObjectURL(shownUrl.current);
    },
    [],
  );
  return empty ? { shown: null, width: null, wrong: null } : drawing;
}

/** A formula or a diagram as a block: the drawing, and its source under it when it is opened. */
function DrawnView(props: NodeViewProps) {
  const { t } = useTranslation();
  const diagram = props.node.type.name === 'diagram';
  const source = String(props.node.attrs.source ?? '');
  const editable = props.editor.isEditable;
  const [open, setOpen] = useState(source === '' && editable);
  const drawing = useDrawing(diagram ? 'diagram' : 'formula', source);
  const what = t(diagram ? 'documents.drawn.diagram' : 'documents.drawn.formula');
  return (
    <NodeViewWrapper className={cx('oc oc-drawn', !editable && 'is-readonly')} data-ot={props.node.type.name}>
      <div
        className="oc-drawing"
        contentEditable={false}
        role={editable ? 'button' : 'img'}
        tabIndex={editable ? 0 : undefined}
        aria-label={editable ? t('documents.drawn.edit', { what }) : what}
        onClick={() => editable && setOpen(true)}
        onKeyDown={(event) => {
          if (editable && (event.key === 'Enter' || event.key === ' ')) {
            event.preventDefault();
            setOpen(true);
          }
        }}
      >
        {drawing.shown === null ? (
          <span className="text-sm text-text-subtle">{source.trim() === '' ? t('documents.drawn.empty', { what }) : t('documents.drawn.drawing')}</span>
        ) : diagram ? (
          <img src={drawing.shown} alt={what} className="mx-auto max-w-full" {...(drawing.width === null ? {} : { style: { width: drawing.width } })} />
        ) : (
          <div dangerouslySetInnerHTML={{ __html: drawing.shown }} />
        )}
      </div>
      {open && editable && (
        <textarea
          className="oc-source"
          value={source}
          aria-label={t('documents.drawn.source', { what })}
          spellCheck={false}
          autoFocus
          onChange={(event) => props.updateAttributes({ source: event.target.value })}
          onBlur={() => setOpen(false)}
          onKeyDown={(event) => {
            if (event.key === 'Escape') {
              event.preventDefault();
              setOpen(false);
              props.editor.commands.focus();
            }
          }}
        />
      )}
      {drawing.wrong !== null && (
        <div contentEditable={false}><StateLine role="alert" tone="danger" compact className="oc-wrong"
          message={drawing.wrong === '' ? t('documents.drawn.invalidSource') : t('documents.drawn.wrong', { message: drawing.wrong })} /></div>
      )}
    </NodeViewWrapper>
  );
}

/** A formula in a line. */
function InlineMathView(props: NodeViewProps) {
  const { t } = useTranslation();
  const source = String(props.node.attrs.source ?? '');
  const drawing = useDrawing('inline', source);
  const editable = props.editor.isEditable;
  const [editing, setEditing] = useState(false);
  return (
    <NodeViewWrapper as="span" className="oi oi-math" data-inline-math="" contentEditable={false}>
      {editing ? (
        <input
          className="rounded-md border border-control-border bg-bg px-3 py-1 font-mono text-sm text-text"
          value={source}
          aria-label={t('documents.drawn.source', { what: t('documents.drawn.formula') })}
          autoFocus
          onChange={(event) => props.updateAttributes({ source: event.target.value })}
          onBlur={() => setEditing(false)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' || event.key === 'Escape') {
              event.preventDefault();
              setEditing(false);
              props.editor.commands.focus();
            }
          }}
        />
      ) : drawing.shown !== null ? (
        <span onClick={() => editable && setEditing(true)} dangerouslySetInnerHTML={{ __html: drawing.shown }} />
      ) : (
        <span className="text-text-subtle" onClick={() => editable && setEditing(true)}>
          {source === '' ? '$ $' : source}
        </span>
      )}
    </NodeViewWrapper>
  );
}

/** The nodes of formulas and diagrams with the views that draw them. */
export const drawnViews: Readonly<Record<string, AnyExtension>> = {
  math: MathBlock.extend({ addNodeView: () => ReactNodeViewRenderer(DrawnView) }),
  diagram: DiagramBlock.extend({ addNodeView: () => ReactNodeViewRenderer(DrawnView) }),
  inlineMath: InlineMath.extend({ addNodeView: () => ReactNodeViewRenderer(InlineMathView, { as: 'span' }) }),
};
