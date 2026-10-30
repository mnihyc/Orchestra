/** The same quiet, explained mark wherever an answer came only through administrator reach. */
import { ShieldUser } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { usePlaceLabel } from '@/features/navigation/placeLabels';
import type { AdminAccess, NodeOrSystem } from '@/generated/api-types';
import { cx } from '@/ui/cx';
import { Tooltip } from '@/ui/Tooltip';

export interface AsAdminMarkProps {
  of: NodeOrSystem | null | undefined;
  iconOnly?: boolean;
  /** Dense table rows retain the shield on a phone. */
  responsive?: boolean;
  /** Page headers say why in visible words, on the same line as Private where appropriate. */
  sentence?: boolean;
  private?: boolean;
}

export function useAdminPlaceName(of: NodeOrSystem | null | undefined): string | null {
  const { t } = useTranslation();
  const label = usePlaceLabel();
  return of === null || of === undefined ? null : of === 'system' ? t('access.place.system') : label(of) ?? t('access.place.hidden');
}

function Mark({ of, iconOnly = false, responsive = false, sentence = false, private: privatePage = false }: AsAdminMarkProps & { of: NodeOrSystem }) {
  const { t } = useTranslation();
  const place = useAdminPlaceName(of);
  const reason = of === 'system' ? t('adminReach.mark.system') : t('adminReach.mark.place', { place });
  const words = privatePage ? t('adminReach.mark.private', { reason }) : reason;
  return <Tooltip content={words}><span role="img" aria-label={words}
    data-as-admin-mark className={cx('inline-flex max-w-full shrink-0 items-center gap-1 text-xs font-normal text-text-muted', sentence && 'text-sm')}>
    <ShieldUser aria-hidden className="size-4 shrink-0" />
    {!iconOnly && <span aria-hidden className={cx(responsive && 'hidden sm:inline')}>{t('adminReach.mode.name')}</span>}
    {sentence && <><span aria-hidden>·</span><span aria-hidden>{words}</span></>}
  </span></Tooltip>;
}

export function AsAdminMark(props: AsAdminMarkProps) {
  return props.of === null || props.of === undefined ? null : <Mark {...props} of={props.of} />;
}

/** Common access, compact rows and chat capabilities all carry the same server fact. */
export function adminAccessOf(value: unknown): AdminAccess | null {
  if (typeof value !== 'object' || value === null) return null;
  const row = value as Record<string, unknown>;
  for (const candidate of [row, row.access, row.can]) {
    if (typeof candidate !== 'object' || candidate === null || !('admin_access' in candidate)) continue;
    const mark = candidate.admin_access;
    if (typeof mark !== 'object' || mark === null || !('place' in mark)) continue;
    const place = mark.place;
    if (place === 'system') return mark as AdminAccess;
    if (typeof place === 'object' && place !== null && 'id' in place && typeof place.id === 'string'
      && 'redacted' in place && typeof place.redacted === 'boolean') return mark as AdminAccess;
  }
  return null;
}

/** Privacy is a separate server fact; native reach alone never means a page is private. */
export function privateAdminAccess(value: unknown): boolean {
  return typeof value === 'object' && value !== null && 'private' in value && value.private === true;
}
