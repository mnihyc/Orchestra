/** The first keyboard stop bypasses the frame's navigation. */
import { useTranslation } from 'react-i18next';
import { mainContent } from '@/shell/areas';

export function SkipMain() {
  const { t } = useTranslation();
  return <a href="#main-content" className="focus-inset sr-only focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:z-(--layer-modal) focus:rounded-md focus:bg-bg focus:px-3 focus:py-2"
    onClick={(event) => { event.preventDefault(); mainContent()?.focus(); }}>{t('shell.skipMain')}</a>;
}
