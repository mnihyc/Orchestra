/** Shared PermissionGate component (access); primitives are wrapped here.
 *
 * What a control does when its reader may not use it: it is left out where it would only be noise, it
 * stays and is switched off with the reason in a tooltip where they would look for it, or a quiet line
 * stands in its place. Which of the three is the screen's to say; whether the reader may is the
 * caller's to know from what the API says they hold on the thing, never from a role's name.
 */
import { cloneElement, isValidElement, type ReactElement, type ReactNode } from 'react';

import { Tooltip } from '@/ui/Tooltip';

export interface PermissionGateProps {
  /** Whether the reader holds what the control needs. */
  allowed: boolean;
  /**
   * `hide` leaves the control out; `block` keeps it, switched off, with `reason` under the pointer
   * and for a screen reader; `note` puts `note` in its place.
   */
  mode?: 'hide' | 'block' | 'note';
  /** For `block`: why the reader may not, and what to do about it. */
  reason?: string;
  /** For `note`: the line that stands in the place of the control. */
  note?: ReactNode;
  /** The control. For `block` it is one element that takes `blocked`, as a `Button` does. */
  children: ReactNode;
}

export function PermissionGate({ allowed, mode = 'hide', reason, note, children }: PermissionGateProps) {
  if (allowed) return children;
  switch (mode) {
    case 'hide':
      return null;
    case 'note':
      return note ?? null;
    case 'block':
      if (!isValidElement<{ blocked?: boolean; 'aria-description'?: string }>(children)) return null;
      return (
        <Tooltip content={reason}>
          {cloneElement(children as ReactElement<{ blocked?: boolean; 'aria-description'?: string }>, {
            blocked: true,
            ...(reason === undefined ? {} : { 'aria-description': reason }),
          })}
        </Tooltip>
      );
  }
}
