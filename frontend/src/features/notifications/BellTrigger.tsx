/** A compact stateful bell: the actual glyph, a visible Off label and the effective level for readers. */
import { Bell, BellOff, BellRing, LoaderCircle } from 'lucide-react';
import { type ButtonHTMLAttributes, type Ref, useId } from 'react';
import { useTranslation } from 'react-i18next';

import type { ChatLevel, DocumentDiscussionLevel, SubscriptionLevel } from '@/generated/api-types';
import { buttonClasses } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { Tooltip } from '@/ui/Tooltip';

interface Props extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'children' | 'aria-label'> {
  label: string;
  /** Distinguishes independent place/chat controls without losing their visible effective state. */
  scopeLabel?: string;
  level: SubscriptionLevel | ChatLevel | DocumentDiscussionLevel;
  inherited?: boolean;
  /** A place bell names its short effective level in the header. */
  showLevel?: boolean;
  pending?: boolean;
  ref?: Ref<HTMLButtonElement>;
}

/** Radix Slot combines description ids; this bell already owns one complete, live state description. */
function BellButton({ stateId, ...props }: ButtonHTMLAttributes<HTMLButtonElement> & { stateId: string; ref?: Ref<HTMLButtonElement> }) {
  return <button {...props} type="button" aria-describedby={stateId} />;
}

export function BellTrigger({ label, scopeLabel, level, inherited = false, showLevel = false, pending = false, disabled = false, ref, className, onClick, onPointerDown, onKeyDown, ...rest }: Props) {
  const { t } = useTranslation();
  const stateId = useId();
  const name = t(`notificationBell.level.${level}`);
  const description = inherited ? t('notificationBell.inherited', { level: name }) : name;
  const Icon = level === 'off' ? BellOff : level === 'all' ? BellRing : Bell;
  const named = showLevel || level === 'off';
  return <Tooltip content={named ? label : description}>
    <BellButton {...rest} ref={ref} stateId={stateId} aria-label={label}
      disabled={disabled} aria-busy={pending || undefined} aria-disabled={pending || undefined}
      className={cx(buttonClasses({ size: 'sm', variant: 'ghost', pending, disabled }), named ? 'gap-1' : 'w-(--control-sm) px-0', className)}
      onClick={(event) => { if (pending) event.preventDefault(); else onClick?.(event); }}
      onPointerDown={(event) => { if (pending) event.preventDefault(); else onPointerDown?.(event); }}
      onKeyDown={(event) => {
        if (pending && ['Enter', ' ', 'ArrowDown', 'ArrowUp'].includes(event.key)) {
          // Keep ordinary Alt-arrow workbench navigation, while no key can reopen a busy menu.
          if (!event.altKey || event.key === 'Enter' || event.key === ' ') event.preventDefault();
          return;
        }
        onKeyDown?.(event);
      }}>
      {pending ? <LoaderCircle className="animate-spin" aria-hidden /> : <Icon aria-hidden />}
      {named && <span>{scopeLabel === undefined ? name : scopeLabel + ' · ' + name}</span>}
      <span id={stateId} className="sr-only">{description}</span>
    </BellButton>
  </Tooltip>;
}
