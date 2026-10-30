/** Shared QrCode component (links); primitives are wrapped here.
 *
 * A link to scan with a phone: an authenticator app is set up by one, a chat bot is connected by one.
 * It is always dark squares on a light tile with its quiet border, in both themes, because a scanner
 * reads dark on light and a dark page would otherwise invert it. A text too long for the largest
 * symbol is not drawn; the caller keeps the text beside it, which is what a person types by hand
 * where scanning is not possible.
 */
import { useMemo } from 'react';

import { qrModules } from '@/lib/qr';
import { cx } from '@/ui/cx';

/** The border of light squares a scanner needs round the symbol; the standard asks for four. */
const QUIET_SQUARES = 4;

export interface QrCodeProps {
  /** What the symbol says. */
  value: string;
  /** What it is, for a screen reader: "Code to scan with your authenticator app". */
  label: string;
  /** Its side in pixels. */
  size?: number;
  className?: string;
}

export function QrCode({ value, label, size = 176, className }: QrCodeProps) {
  const modules = useMemo(() => qrModules(value), [value]);
  if (modules === null) return null;
  const side = modules.length + QUIET_SQUARES * 2;
  // One path for all the dark squares: a symbol of a few thousand squares stays a single element.
  const path = modules
    .flatMap((row, y) =>
      row.flatMap((dark, x) => (dark ? [`M${x + QUIET_SQUARES} ${y + QUIET_SQUARES}h1v1h-1z`] : [])),
    )
    .join('');
  return (
    <svg
      role="img"
      aria-label={label}
      viewBox={`0 0 ${side} ${side}`}
      width={size}
      height={size}
      shapeRendering="crispEdges"
      className={cx('shrink-0 rounded-md border border-border', className)}
    >
      {/* The light tile is part of the picture and is drawn here, not styled: no theme may take it away. */}
      <rect width={side} height={side} fill="#fff" />
      <path d={path} fill="#000" />
    </svg>
  );
}
