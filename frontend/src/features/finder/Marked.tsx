/** A name with the letters that were typed marked in it, so that a reader sees why it was found. */
import { Fragment } from 'react';

export interface MarkedProps {
  text: string;
  /** [start, end) in UTF-16 units, in order and not overlapping. */
  ranges: readonly (readonly [number, number])[];
}

export function Marked({ text, ranges }: MarkedProps) {
  if (ranges.length === 0) return text;
  const parts: { text: string; marked: boolean }[] = [];
  let at = 0;
  for (const [from, to] of ranges) {
    if (from > at) parts.push({ text: text.slice(at, from), marked: false });
    parts.push({ text: text.slice(from, to), marked: true });
    at = to;
  }
  if (at < text.length) parts.push({ text: text.slice(at), marked: false });
  return (
    <>
      {parts.map((part, index) => (
        // The parts of a name never change while it is shown: their place is their name.
        <Fragment key={index}>
          {part.marked ? <mark className="rounded-sm bg-accent-subtle px-px text-text">{part.text}</mark> : part.text}
        </Fragment>
      ))}
    </>
  );
}
