/** A server-marked private opening stays explicit, without asking the administrator to grant themselves access. */
import type { NodeOrSystem } from '@/generated/api-types';
import { PrivateOpeningMark } from '@/ui/AsAdminMark';
import { useAdminOpening } from '@/features/access/opening';

export function AdminReachNotice({ of, private: privatePage = false, object }: { of: NodeOrSystem | null | undefined; private?: boolean; object?: { kind: string; id: string } }) {
  const reached = of !== null && of !== undefined;
  useAdminOpening(object?.kind ?? '', object?.id, reached);
  return privatePage && reached ? <PrivateOpeningMark of={of} /> : null;
}
