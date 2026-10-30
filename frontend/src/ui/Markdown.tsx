/** Shared Markdown component (content); primitives are wrapped here.
 *
 * Text written as Markdown, drawn in the type of a message. While text is still arriving it is read
 * again at most every 50 ms, however fast it grows, and a caret follows its end.
 */
import { memo, useEffect, useMemo, useRef, useState } from 'react';

import { lexMarkdown, renderBlocks } from '@/lib/markdown';
import { cx } from '@/ui/cx';

/** How often text that is still arriving is read again: often enough to look continuous, rarely enough to stay cheap. */
export const STREAM_REPARSE_MS = 50;

export interface MarkdownProps {
  text: string;
  /** The text is still arriving: it is read at a steady pace, and a caret follows it. */
  streaming?: boolean;
  /** Tighter spacing, for a step's output and a sub-agent's lane. */
  compact?: boolean;
  /** What a person wrote: its line breaks stay line breaks. */
  breaks?: boolean;
  className?: string;
}

/** The text as it stood at most `pauseMs` ago, for as long as `active`; the text itself at once when it is not. */
function useSteady(text: string, active: boolean, pauseMs: number): string {
  const [steady, setSteady] = useState(text);
  const last = useRef(0);
  useEffect(() => {
    if (!active) return;
    // Each change aims at the same moment, a pause after the last time the text was read, and the
    // newest text is what is read then: a stream faster than the pause is read once a pause.
    const timer = setTimeout(
      () => {
        last.current = Date.now();
        setSteady(text);
      },
      Math.max(0, last.current + pauseMs - Date.now()),
    );
    return () => clearTimeout(timer);
  }, [text, active, pauseMs]);
  return active ? steady : text;
}

function MarkdownText({ text, streaming = false, compact = false, breaks = false, className }: MarkdownProps) {
  const steady = useSteady(text, streaming, STREAM_REPARSE_MS);
  const blocks = useMemo(() => {
    const tail = streaming ? { node: <span aria-hidden className="ml-0.5 inline-block h-[1em] w-0.5 translate-y-0.5 animate-pulse bg-text-muted" /> } : undefined;
    return renderBlocks(lexMarkdown(steady, breaks), 'm', tail);
  }, [steady, streaming, breaks]);
  return <div data-user-content className={cx('flex min-w-0 flex-col text-md text-text', compact ? 'gap-1' : 'gap-3', className)}>{blocks}</div>;
}

export const Markdown = memo(MarkdownText);
