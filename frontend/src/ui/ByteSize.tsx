/** Shared ByteSize component (utilities); primitives are wrapped here.
 *
 * A size in bytes, in binary units, in the reader's language.
 */
import { useAppearance } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { cx } from '@/ui/cx';

export function ByteSize({ bytes, className }: { bytes: number; className?: string | undefined }) {
  const locale = useAppearance((state) => state.locale);
  return <span className={cx('tabular-nums', className)}>{formatBytes(bytes, locale)}</span>;
}
