/** The round button above the composer that goes to the end of the conversation, with the count of rows that came while the reader was elsewhere. */
import { ArrowDown } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { IconButton } from '@/ui/IconButton';

export function FollowButton({ away, onClick }: { away: number; onClick: () => void }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const name = away > 0 ? t('timeline.newBelow', { count: away }) : t('timeline.jump');
  return (
    <div className="pointer-events-none absolute inset-x-0 bottom-3 z-(--layer-sticky) flex justify-center">
      <span className="pointer-events-auto relative">
        <IconButton variant="secondary" label={name} icon={<ArrowDown aria-hidden />} className="rounded-full shadow-md" onClick={onClick} data-follow-button />
        {away > 0 && <span aria-hidden className="pointer-events-none absolute -top-1 -right-1 min-w-5 rounded-full bg-accent-solid px-1 text-center text-xs leading-5 text-text-on-accent">{formatNumber(away, locale)}</span>}
      </span>
    </div>
  );
}
