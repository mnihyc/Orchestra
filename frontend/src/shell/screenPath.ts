/** The authorized parents of a screen; its own name belongs in the heading and tab. */
import type { ScreenHeading } from '@/shell/screen';

export interface ScreenStep {
  id: string;
  label: string;
  /** Missing only where the server redacted a place. */
  href?: string | undefined;
}
export type ScreenPath = readonly ScreenStep[];

/** A node's own page drops that node; a document or sub-screen keeps the place it belongs to. */
export function screenPath(heading: ScreenHeading, authorized: ScreenPath, trail: ScreenPath): ScreenPath {
  return [...(heading.leaf === true ? authorized : authorized.slice(0, -1)), ...trail];
}
