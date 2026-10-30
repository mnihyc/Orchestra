/** A server-marked private opening stays explicit, without asking the administrator to grant themselves access. */
import type { NodeOrSystem } from '@/generated/api-types';
import { AsAdminMark } from '@/ui/AsAdminMark';
import { useAdminOpening } from '@/features/access/opening';

export function AdminReachNotice({ of, private: privatePage = false, object }: { of: NodeOrSystem | null | undefined; private?: boolean; object?: { kind: string; id: string } }) {
  useAdminOpening(object?.kind ?? '', object?.id, of !== null && of !== undefined);
  return <AsAdminMark of={of} sentence private={privatePage} />;
}
