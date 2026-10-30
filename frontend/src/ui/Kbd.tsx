/** Shared Kbd component (utilities); primitives are wrapped here.
 *
 * A shortcut as the keys printed on the keyboard in front of its reader: the Command sign on Apple's
 * systems, "Ctrl" elsewhere; two keys pressed one after another stand apart.
 */
import { Fragment } from 'react';
import { useTranslation } from 'react-i18next';

import { shortcutCaps } from '@/lib/shortcuts';
import { cx } from '@/ui/cx';

export interface KbdProps {
  /** As it is written: `Mod+K`, `Shift+F6`, `G I`. */
  shortcut: string;
  /** `inverse` on a dark-on-light ground turned round, as in a tooltip. */
  tone?: 'plain' | 'inverse';
  className?: string;
}

export function Kbd({ shortcut, tone = 'plain', className }: KbdProps) {
  const { t } = useTranslation();
  const chords = shortcutCaps(shortcut);
  return (
    <span className={cx('inline-flex items-center gap-1 whitespace-nowrap', className)}>
      {chords.map((caps, chord) => (
        // A shortcut never changes while it is shown, so the place of a key in it names the key.
        <Fragment key={chord}>
          {chord > 0 && <span className="text-xs">{t('ui.kbd.then')}</span>}
          <span className="inline-flex items-center gap-0.5">
            {caps.map((cap, index) => (
              <kbd
                key={index}
                className={cx(
                  'inline-flex h-5 min-w-5 items-center justify-center rounded-sm border px-1 font-sans text-xs font-medium',
                  tone === 'plain' ? 'border-border-strong bg-bg-subtle text-text-muted' : 'border-current/30',
                )}
              >
                {cap}
              </kbd>
            ))}
          </span>
        </Fragment>
      ))}
    </span>
  );
}
