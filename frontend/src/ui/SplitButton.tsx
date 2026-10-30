/** Shared SplitButton component (actions); primitives are wrapped here.
 *
 * One act with a few others beside it: the button does the usual one, and the arrow opens the list of
 * the rest. Both stay in the tab order. While the act is under way the main button shows it.
 */
import { ChevronDown } from 'lucide-react';
import type { ReactNode } from 'react';

import { Button, type ButtonVariant } from '@/ui/Button';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { Tooltip } from '@/ui/Tooltip';

export interface SplitButtonProps {
  children: ReactNode;
  onClick: () => void;
  entries: readonly MenuEntry[];
  /** What the list is of, and the arrow's name. */
  menuLabel: string;
  variant?: ButtonVariant;
  size?: 'sm' | 'md';
  icon?: ReactNode;
  pending?: boolean;
  disabled?: boolean;
  /** The menu can stay available when the primary action has a different readiness condition. */
  menuDisabled?: boolean;
  tutorialTarget?: string;
  /** Why the main act cannot be done now, shown on the button. */
  reason?: string | undefined;
  className?: string;
}

export function SplitButton({ children, onClick, entries, menuLabel, variant = 'primary', size = 'md', icon, pending = false, disabled = false, menuDisabled, tutorialTarget, reason, className }: SplitButtonProps) {
  return (
    <span className={`inline-flex ${className ?? ''}`}>
      <Tooltip content={reason}>
        <Button
          variant={variant}
          size={size}
          {...(icon === undefined ? {} : { icon })}
          pending={pending}
          disabled={disabled}
          data-tutorial-target={tutorialTarget}
          className="rounded-r-none"
          onClick={onClick}
        >
          {children}
        </Button>
      </Tooltip>
      <Menu
        label={menuLabel}
        align="end"
        side="top"
        entries={entries}
        trigger={
          <Button variant={variant} size={size} disabled={menuDisabled ?? disabled} aria-label={menuLabel} className="rounded-l-none border-l border-text-on-accent/30 px-1">
            <ChevronDown aria-hidden />
          </Button>
        }
      />
    </span>
  );
}
