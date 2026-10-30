/** A runtime update's optional notification uses the shared people-count inspection. */
import { useTranslation } from 'react-i18next';
import type { UpdateAudience } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { PeopleCount } from '@/ui/AvatarStack';
import { Checkbox } from '@/ui/Checkbox';

export function UpdateNotify({ audience, checked, onChange, disabled = false }: { audience: UpdateAudience | null | undefined; checked: boolean; onChange: (checked: boolean) => void; disabled?: boolean }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const count = audience === undefined || audience === null ? 0 : audience.people.length + audience.hidden;
  if (audience === undefined || audience === null || count === 0) return null;
  const label = t('programVersions.notify', { count, number: formatNumber(count, locale) });
  return <span className="mr-auto flex min-w-0 flex-wrap items-center gap-1"><Checkbox label={<span className="sr-only">{label}</span>} checked={checked} onCheckedChange={onChange} disabled={disabled} /><PeopleCount users={audience.people} total={count}>{label}</PeopleCount></span>;
}
