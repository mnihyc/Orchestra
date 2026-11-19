/** Shared IconButton component (actions); primitives are wrapped here.
 *
 * A button that shows an icon alone. It always has a name: the name is what a screen reader says and
 * what the pointer reads when it rests on the button, with the shortcut beside it when there is one.
 */
import { LoaderCircle } from 'lucide-react';
import type { ButtonHTMLAttributes, MouseEvent, ReactNode, Ref } from 'react';

import { ActionIcon, actionForLabel, type ActionId } from '@/ui/actions';
import { cx } from '@/ui/cx';
import { Tooltip } from '@/ui/Tooltip';

export interface IconButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'aria-label' | 'children'> {
  /** What the button does, in words. */
  label: string;
  action?: ActionId;
  icon?: ReactNode;
  variant?: 'ghost' | 'secondary';
  size?: 'sm' | 'md';
  /** The shortcut that does the same, shown beside the name: `Mod+K`. */
  shortcut?: string;
  /** Where the name appears. */
  side?: 'top' | 'right' | 'bottom' | 'left';
  pending?: boolean;
  /** Unavailable but focusable, so its reason can be read. */
  blocked?: boolean;
  /** The one hover/focus explanation; otherwise its accessible name is shown. */
  reason?: string | undefined;
  /** Pressed, for a button that switches something on and off. */
  pressed?: boolean;
  ref?: Ref<HTMLButtonElement>;
}

const SIZES = { sm: 'size-(--control-sm)', md: 'size-(--control-md)' };

const LOOKS = {
  ghost: { rest: 'text-text-muted', live: 'hover:bg-bg-muted hover:text-text', on: 'bg-bg-muted text-text' },
  secondary: {
    rest: 'border border-control-border bg-surface text-text',
    live: 'hover:bg-bg-subtle',
    on: 'bg-bg-muted text-text',
  },
};

/** An explicit state/value description stays complete after the tooltip's Slot merges its own id. */
function IconButtonFace({ ownerDescription, children, ref, ...props }: ButtonHTMLAttributes<HTMLButtonElement> & { ownerDescription?: string | undefined; ref?: Ref<HTMLButtonElement> }) {
  return <button {...props} ref={ref} aria-describedby={ownerDescription ?? props['aria-describedby']}>{children}</button>;
}

export function IconButton({
  label,
  action,
  icon,
  variant = 'ghost',
  size = 'md',
  shortcut,
  side = 'bottom',
  pending = false,
  blocked = false,
  reason,
  pressed,
  disabled = false,
  className,
  onClick,
  ref,
  ...rest
}: IconButtonProps) {
  const act = action ?? actionForLabel(label);
  const look = LOOKS[variant];
  const press = (event: MouseEvent<HTMLButtonElement>) => {
    if (pending || blocked) {
      event.preventDefault();
      return;
    }
    onClick?.(event);
  };

  return (
    <Tooltip content={reason ?? label} shortcut={shortcut} side={side}>
      <IconButtonFace
        ownerDescription={reason === undefined ? rest['aria-describedby'] : undefined}
        ref={ref}
        type="button"
        aria-label={label}
        data-action={act}
        aria-pressed={pressed}
        aria-disabled={pending || blocked || undefined}
        aria-busy={pending || undefined}
        disabled={disabled}
        className={cx(
          'inline-flex shrink-0 items-center justify-center rounded-md transition-colors',
          SIZES[size],
          disabled || blocked ? 'cursor-not-allowed text-text-disabled' : look.rest,
          !disabled && !blocked && !pending && look.live,
          pressed === true && !disabled && look.on,
          className,
        )}
        onClick={press}
        {...rest}
      >
        {pending ? <LoaderCircle className="animate-spin" aria-hidden /> : act === undefined ? icon : <ActionIcon action={act} />}
      </IconButtonFace>
    </Tooltip>
  );
}
