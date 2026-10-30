/** Shared CodeBlock component (content); primitives are wrapped here.
 *
 * A block of code with its language named, a button that copies it whole and a switch for wrapping
 * long lines. It is drawn in mono type on the inset ground; the language is shown, and colours
 * follow it once the highlighter for that language has arrived (it is fetched when first needed).
 */
import { WrapText } from 'lucide-react';
import { memo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { CopyButton } from '@/ui/CopyButton';
import { cx } from '@/ui/cx';
import { IconButton } from '@/ui/IconButton';
import { PREVIEW_LINES, Preview } from '@/ui/Preview';

export interface CodeBlockProps {
  code: string;
  /** The language it is written in, as the writer named it. */
  lang?: string | null;
  /** Legacy callers remain accepted; long blocks now expand in the page instead of scrolling vertically. */
  maxHeight?: string;
  className?: string;
}

function CodeBlockView({ code, lang = null, className }: CodeBlockProps) {
  const { t } = useTranslation();
  const [wrap, setWrap] = useState(false);
  const language = lang === null || lang.trim() === '' ? null : (lang.trim().split(/\s+/)[0] ?? null);
  const lines = code.split('\n');
  return (
    <figure className={cx('group/code min-w-0 overflow-hidden rounded-lg border border-border bg-bg-inset text-text', className)}>
      <figcaption className="flex min-h-(--caption-height) items-center justify-between gap-2 border-b border-border px-3 text-xs text-text-muted">
        <span data-overflow-text={language ?? t('ui.code.plain')} className="truncate">{language ?? t('ui.code.plain')}</span>
        <span className="flex items-center gap-0.5">
          <IconButton
            size="sm"
            label={t('ui.code.wrap')}
            pressed={wrap}
            icon={<WrapText aria-hidden />}
            onClick={() => setWrap((on) => !on)}
          />
          <CopyButton value={code} label={t('ui.code.copy')} />
        </span>
      </figcaption>
      <Preview more={lines.length > PREVIEW_LINES}>{(expanded) => <pre
        role="region"
        tabIndex={0}
        aria-label={language === null ? t('ui.code.plain') : t('ui.code.label', { language })}
        data-content-role="source"
        data-scroll-allow="code"
        className={cx('focus-inset overflow-x-auto p-3 text-sm', wrap ? 'break-words whitespace-pre-wrap' : 'whitespace-pre')}
      >
        <code>{expanded ? code : lines.slice(0, PREVIEW_LINES).join('\n')}</code>
      </pre>}</Preview>
    </figure>
  );
}

export const CodeBlock = memo(CodeBlockView);

