/** Shared NodeIcon component (status); primitives are wrapped here.
 *
 * The icon of a place wherever one is drawn: the glyph of its kind, in a colour. An org, a project, a
 * folder and a topic each have a glyph of their own and start in the colour of their kind, so that the
 * kinds are told apart at a glance; whoever manages one may give it another of the twelve, or a colour of
 * its own, and it is then drawn in that one everywhere, for everybody. A colour of its own is kept legible on
 * both grounds: no lighter than reads on the light ones, no darker than reads on the dark ones (`node-own`
 * in the tokens). A topic that is done carries a check in its glyph.
 * A thread and a document have a quiet glyph and no colour: what a thread is doing is said by the icon
 * of its state, which is not this one.
 *
 * The colour is never the only sign: the shape says the kind and the check says done. Where no name
 * stands beside the icon that says so, `label` gives it one for a screen reader; otherwise it is
 * decoration and hidden from one.
 */
import { Briefcase, createLucideIcon, Diamond, FileText, Folder, LayoutGrid, type LucideIcon, MessageSquareText } from 'lucide-react';
import type { CSSProperties } from 'react';

import { cx } from '@/ui/cx';

/** What an icon can be of: the four kinds of place (a project that lies in a project is a folder), a thread and a document. */
export type NodeIconKind = 'org' | 'project' | 'folder' | 'topic' | 'thread' | 'document';

/** The twelve colours a place can be given, by name, in the order of the palette: a colour is always said in words as well. The first is colour 1. */
export const NODE_COLOR_NAMES = ['red', 'orange', 'amber', 'lime', 'green', 'teal', 'cyan', 'blue', 'indigo', 'purple', 'pink', 'grey'] as const;

/** How many colours a place can be given: the twelve of the design system, counted from 1. */
export const NODE_COLORS = NODE_COLOR_NAMES.length;

/**
 * A topic that is done: the topic's own diamond with a check inside it. The check is the one the icon
 * set draws inside its circle, moved down by half a unit, where the diamond is widest.
 */
const DiamondCheck = createLucideIcon('diamond-check', [
  ['path', { d: 'M2.7 10.3a2.41 2.41 0 0 0 0 3.41l7.59 7.59a2.41 2.41 0 0 0 3.41 0l7.59-7.59a2.41 2.41 0 0 0 0-3.41l-7.59-7.59a2.41 2.41 0 0 0-3.41 0Z', key: 'outline' }],
  ['path', { d: 'm15.5 10-4.5 4.5L8.5 12', key: 'check' }],
]);

/** The glyph of each kind. A project has one that is no folder, since a folder is what a project inside a project is called. */
export const NODE_GLYPHS: Record<NodeIconKind, LucideIcon> = {
  org: LayoutGrid,
  project: Briefcase,
  folder: Folder,
  topic: Diamond,
  thread: MessageSquareText,
  document: FileText,
};

/**
 * The colour each kind of place starts in, of the twelve: purple for an org, blue for a project, amber
 * for a folder (the colour folders have wherever files are kept) and teal for a topic. Amber is far
 * from the other three on the wheel and those stand an eighth of it apart, which is as far as four of
 * the twelve can be while none is the red, the orange or the green in which the state of work is
 * drawn. Each kind also has a glyph of its own, which is the sign that does not depend on colour.
 */
export const KIND_COLORS: Record<'org' | 'project' | 'folder' | 'topic', number> = { org: 10, project: 8, folder: 3, topic: 6 };

/** One class for each of the twelve, written out so that the stylesheet holds them all. */
const INKS = [
  'text-(--node-ink-1)',
  'text-(--node-ink-2)',
  'text-(--node-ink-3)',
  'text-(--node-ink-4)',
  'text-(--node-ink-5)',
  'text-(--node-ink-6)',
  'text-(--node-ink-7)',
  'text-(--node-ink-8)',
  'text-(--node-ink-9)',
  'text-(--node-ink-10)',
  'text-(--node-ink-11)',
  'text-(--node-ink-12)',
] as const;

/** A colour of a place's own as the server keeps it: `#` and six hexadecimal digits. */
const OWN_COLOR = /^#[0-9a-f]{6}$/i;

/** Whether a text is a colour of a place's own. */
export function isOwnColor(color: unknown): color is string {
  return typeof color === 'string' && OWN_COLOR.test(color);
}

/** Whether a number is one of the twelve. */
export function isNodeColor(color: unknown): color is number {
  return typeof color === 'number' && Number.isInteger(color) && color >= 1 && color <= NODE_COLORS;
}

/** The colour a place is drawn in: the one it was given, or its kind's; `null` for a kind that has none. */
export function nodeColor(kind: NodeIconKind, color: number | null | undefined): number | null {
  if (kind === 'thread' || kind === 'document') return null;
  return isNodeColor(color) ? color : KIND_COLORS[kind];
}

/** The class that draws a glyph in one of the twelve; the quiet colour of text for a number that is none of them. */
export function nodeInk(color: number | null): string {
  return (color === null ? undefined : INKS[color - 1]) ?? 'text-text-subtle';
}

export interface NodeIconProps {
  kind: NodeIconKind;
  /** The colour somebody gave the place, 1 to 12; `null` or left out for the colour of its kind. */
  color?: number | null | undefined;
  /** A colour of the place's own, `#rrggbb`, which it is drawn in instead of the twelve's. */
  own?: string | null | undefined;
  /** A topic that is done: its glyph carries a check. */
  done?: boolean;
  /** Says what the icon is where nothing beside it does; left out, the icon is decoration. */
  label?: string;
  /** The side of the glyph in pixels; left out, the size icons have everywhere. */
  size?: number;
  className?: string;
}

export function NodeIcon({ kind, color, own, done = false, label, size, className }: NodeIconProps) {
  const Glyph = kind === 'topic' && done ? DiamondCheck : NODE_GLYPHS[kind];
  const custom = kind !== 'thread' && kind !== 'document' && isOwnColor(own) ? own : null;
  const ink = custom === null ? nodeInk(nodeColor(kind, color)) : 'node-own';
  return (
    <span
      {...(label === undefined ? { 'aria-hidden': true } : { role: 'img', 'aria-label': label })}
      data-node-icon={kind}
      data-node-color={custom === null ? (nodeColor(kind, color) ?? undefined) : 'own'}
      {...(custom === null ? {} : { style: { '--node-own': custom } as CSSProperties })}
      className={cx('inline-flex shrink-0 items-center justify-center', ink, className)}
    >
      <Glyph aria-hidden {...(size === undefined ? {} : { size })} />
    </span>
  );
}
