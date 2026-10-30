/** Explicit adjacent pages for one immutable directory observation; no invented total. */
import { ChevronLeft, ChevronRight, RotateCcw } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { DirectoryWindow } from '@/realtime/stores/environment';
import { IconButton } from '@/ui/IconButton';

export function DirectoryPager({ window, path, navigate }: {
  window: DirectoryWindow;
  path: string;
  navigate: (path: string, action: 'previous' | 'next' | 'restart') => void;
}) {
  const { t } = useTranslation();
  const ordinal = window.page?.ordinal ?? 1;
  // Page one already exposes its failure/reload beside the rows. Later pages need the
  // separate way back because their old observation can remain visible after invalidation.
  if (ordinal === 1 && !window.canNext) return null;
  const busy = window.navigation === 'loading';
  return <span className="inline-flex shrink-0 items-center gap-0.5" role="group" aria-label={t('files.pages.label', { path: path || '/' })}>
    <IconButton size="sm" label={t('files.pages.previous')} icon={<ChevronLeft aria-hidden />} disabled={busy || !window.canPrevious} onClick={() => navigate(path, 'previous')} />
    <span className="whitespace-nowrap px-1 text-xs tabular-nums" role="status">{t('files.pages.number', { number: ordinal })}</span>
    <IconButton size="sm" label={t('files.pages.next')} icon={<ChevronRight aria-hidden />} disabled={busy || !window.canNext} onClick={() => navigate(path, 'next')} />
    {(window.restartRequired || window.capacity !== null) && <IconButton size="sm" label={t('files.pages.restart')} icon={<RotateCcw aria-hidden />} disabled={busy} onClick={() => navigate(path, 'restart')} />}
  </span>;
}
