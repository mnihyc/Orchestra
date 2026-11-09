/** Shared CopyButton component (utilities); primitives are wrapped here.
 *
 * Copies a text and says that it did, with a tick that stays for a moment. Where the browser does
 * not let the page write to the clipboard, the button says that instead of pretending.
 */
import { Check, Copy, X } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { writeToClipboard } from '@/ui/clipboard';
import { cx } from '@/ui/cx';
import { Tooltip } from '@/ui/Tooltip';
import './copy-button.css';

/** How long the tick stays: long enough to be seen, short enough that a second copy is confirmed anew. */
export const COPIED_SHOWN_MS = 1_500;

export interface CopyButtonProps {
  /** What is copied. */
  value: string;
  /** What the button is called: "Copy request id". */
  label: string;
  className?: string;
}

export function CopyButton({ value, label, className }: CopyButtonProps) {
  const { t } = useTranslation();
  const [state, setState] = useState<'idle' | 'copied' | 'refused'>('idle');

  useEffect(() => {
    if (state === 'idle') return;
    const timer = setTimeout(() => setState('idle'), COPIED_SHOWN_MS);
    return () => clearTimeout(timer);
  }, [state]);

  const copy = async () => {
    setState((await writeToClipboard(value)) ? 'copied' : 'refused');
  };

  const said = state === 'copied' ? t('common.copied') : state === 'refused' ? t('common.copyRefused') : label;
  return (
    <Tooltip content={said}>
      <button
        type="button"
        data-copy-control=""
        aria-label={label}
        className={cx(
          'inline-grid size-(--control-sm) shrink-0 items-center justify-items-center rounded-sm text-text transition-colors hover:bg-bg-muted [&>svg]:col-start-1 [&>svg]:row-start-1',
          className,
        )}
        onClick={() => void copy()}
      >
        {/* The control exposes the inherited text baseline while the sign stays centred in its full target. */}
        <span aria-hidden className="invisible col-start-1 row-start-1 select-none">&nbsp;</span>
        {state === 'copied' ? (
          <Check className="text-success" aria-hidden />
        ) : state === 'refused' ? (
          <X className="text-danger" aria-hidden />
        ) : (
          <Copy aria-hidden />
        )}
        <span role="status" className="sr-only">
          {state === 'idle' ? '' : said}
        </span>
      </button>
    </Tooltip>
  );
}
