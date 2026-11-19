/** Shared Button component (actions); primitives are wrapped here.
 *
 * A button says what it does. It has five looks (primary for the one act a screen is for, secondary,
 * ghost for what stands in a row or a bar, danger for what destroys, and the quiet form of danger for
 * a refusal that stands beside an allowance) and two heights.
 *
 * While its act is under way a button shows it and takes no second press: a slow answer is never
 * pressed twice. It keeps the focus meanwhile, which a button that is switched off would lose.
 */
import { LoaderCircle } from 'lucide-react';
import { Slot } from 'radix-ui';
import { cloneElement, isValidElement, type ButtonHTMLAttributes, type MouseEvent, type ReactNode, type Ref } from 'react';

import { ActionIcon, actionForLabel, actionName, type ActionId } from '@/ui/actions';
import { cx } from '@/ui/cx';
import { AdminActTooltip, type AdminActSpec } from '@/ui/AdminAct';

export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger' | 'danger-ghost';
export type ButtonSize = 'sm' | 'md';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  admin?: AdminActSpec;
  /** An ordinary act takes its name and sign from the shared catalogue. */
  action?: ActionId;
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** An icon before the label. */
  icon?: ReactNode;
  /** The act is under way: the button shows it and cannot be pressed again until it is answered. */
  pending?: boolean;
  /**
   * The act is not for this reader, and the screen says why beside it or in a tooltip. It looks like a
   * button that is switched off but keeps the keyboard and the pointer, which a switched-off one loses,
   * so that the reason can be read where the button is.
   */
  blocked?: boolean;
  /** Takes the whole width of what it stands in. */
  block?: boolean;
  /** Gives the look of a button to the one child, which is then what is rendered: a link, as a rule. */
  asChild?: boolean;
  ref?: Ref<HTMLButtonElement>;
}

const BASE =
  'relative inline-flex shrink-0 items-center justify-center gap-2 rounded-md font-medium whitespace-nowrap select-none transition-colors';

const SIZES: Record<ButtonSize, string> = {
  sm: 'h-(--control-sm) px-2 text-sm',
  md: 'h-(--control-md) px-3 text-base',
};

const LOOKS: Record<ButtonVariant, { rest: string; live: string }> = {
  primary: { rest: 'bg-accent-solid text-text-on-accent', live: 'hover:bg-accent-solid-hover' },
  secondary: {
    rest: 'border border-control-border bg-surface text-text',
    live: 'hover:bg-bg-subtle active:bg-bg-muted',
  },
  ghost: { rest: 'text-text-muted', live: 'hover:bg-bg-muted hover:text-text active:bg-bg-muted' },
  danger: { rest: 'bg-danger-solid text-text-on-accent', live: 'hover:bg-danger-solid-hover' },
  // A refusal offered beside an allowance: quiet until it is aimed at.
  'danger-ghost': { rest: 'text-danger-text', live: 'hover:bg-danger-subtle active:bg-danger-subtle' },
};

/** What a button that cannot be pressed looks like, whatever it would look like otherwise. */
const OFF = 'cursor-not-allowed border border-transparent bg-bg-muted text-text-disabled';

/** The classes of a button, for the few things that look like one and are none (a link that is an act). */
export function buttonClasses(options: {
  /** An ordinary act takes its name and sign from the shared catalogue. */
  action?: ActionId;
  variant?: ButtonVariant;
  size?: ButtonSize;
  block?: boolean;
  disabled?: boolean;
  pending?: boolean;
}): string {
  const look = LOOKS[options.variant ?? 'secondary'];
  return cx(
    BASE,
    SIZES[options.size ?? 'md'],
    options.block === true && 'w-full',
    options.disabled === true ? OFF : look.rest,
    options.disabled !== true && options.pending !== true && look.live,
    options.pending === true && 'cursor-progress',
  );
}

export function Button({
  admin,
  action,
  variant = 'secondary',
  size = 'md',
  icon,
  pending = false,
  blocked = false,
  block = false,
  asChild = false,
  disabled = false,
  type = 'button',
  className,
  children,
  onClick,
  ref,
  ...rest
}: ButtonProps) {
  const act = action ?? actionForLabel(children);
  const sign = act === undefined ? icon : <ActionIcon action={act} />;
  const words = children ?? (act === undefined ? null : actionName(act));
  const classes = cx(buttonClasses({ variant, size, block, disabled: disabled || blocked, pending }), className);

  if (asChild) {
    const child = isValidElement<{ children?: ReactNode }>(children) && typeof children.props.children === 'string' ? children : null;
    const childAct = child === null ? undefined : action ?? actionForLabel(child.props.children);
    const childSign = childAct === undefined ? icon : <ActionIcon action={childAct} />;
    const content = child !== null && childSign !== undefined ? cloneElement(child, { children: <>{childSign}{child.props.children}</> }) : children;
    return (
      <AdminActTooltip spec={admin}><Slot.Root ref={ref} className={classes} data-action={childAct ?? act} {...rest}>
        {content}
      </Slot.Root></AdminActTooltip>
    );
  }

  const press = (event: MouseEvent<HTMLButtonElement>) => {
    if (pending || blocked) {
      // Still focused, still announced, and deaf to a second press, or to one that is not for this reader.
      event.preventDefault();
      return;
    }
    onClick?.(event);
  };

  return (
    <AdminActTooltip spec={admin}><button
      ref={ref}
      type={type}
      className={classes}
      disabled={disabled}
      aria-disabled={pending || blocked || undefined}
      data-action={act}
      aria-busy={pending || undefined}
      onClick={press}
      {...rest}
    >
      {pending ? <LoaderCircle className="animate-spin" aria-hidden /> : sign}
      {words}
    </button></AdminActTooltip>
  );
}
