/** Shared AnsiText component (content); primitives are wrapped here.
 *
 * Output read as text with its colours: what a command printed, never a terminal. The colours are
 * taken from the page's own text colour and mixed towards the hue that was asked for, so that they
 * stay readable on the light ground and on the dark one alike.
 */
import { memo, type CSSProperties, useMemo } from 'react';

import { type AnsiColor, type AnsiSpan, parseAnsi } from '@/lib/ansi';
import { cx } from '@/ui/cx';

/** The sixteen named colours, as the token each is mixed from. */
const NAMED: readonly string[] = [
  'var(--text-muted)', // black
  'var(--danger)', // red
  'var(--success)', // green
  'var(--warning)', // yellow
  'var(--info)', // blue
  'var(--identity-9, #7e22ce)', // magenta
  'var(--identity-6, #0e7490)', // cyan
  'var(--text)', // white
];

/** How much of the hue is kept when it is mixed into the text colour: enough to tell, little enough to read. */
const HUE_SHARE = 55;

function colorOf(color: AnsiColor, bright: boolean): string {
  if (color.kind === 'rgb') return `color-mix(in srgb, rgb(${color.r} ${color.g} ${color.b}) ${HUE_SHARE}%, var(--text))`;
  const base = NAMED[color.index % 8] ?? 'var(--text)';
  // The bright eight keep more of the hue.
  return `color-mix(in srgb, ${base} ${bright || color.index >= 8 ? 75 : HUE_SHARE}%, var(--text))`;
}

function styleOf(span: AnsiSpan): CSSProperties | undefined {
  const { fg, bg, bold, dim, italic, underline, inverse } = span.style;
  const style: CSSProperties = {};
  const foreground = inverse === true ? bg : fg;
  const background = inverse === true ? fg : bg;
  if (foreground !== undefined) style.color = colorOf(foreground, bold === true);
  else if (inverse === true) style.color = 'var(--bg)';
  if (background !== undefined) style.backgroundColor = colorOf(background, false);
  else if (inverse === true) style.backgroundColor = 'var(--text)';
  if (bold === true) style.fontWeight = 600;
  if (dim === true) style.opacity = 0.7;
  if (italic === true) style.fontStyle = 'italic';
  if (underline === true) style.textDecoration = 'underline';
  return Object.keys(style).length === 0 ? undefined : style;
}

export interface AnsiTextProps {
  text: string;
  className?: string;
}

function AnsiTextView({ text, className }: AnsiTextProps) {
  const spans = useMemo(() => parseAnsi(text), [text]);
  return (
    <span className={cx('font-mono', className)}>
      {spans.map((span, index) => {
        const style = styleOf(span);
        return style === undefined ? span.text : <span key={index} style={style}>{span.text}</span>;
      })}
    </span>
  );
}

export const AnsiText = memo(AnsiTextView);
