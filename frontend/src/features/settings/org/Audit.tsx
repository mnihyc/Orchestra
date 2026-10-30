/** The org's audit trail, with a override marked.
 * The same filtered trail is used by an org, a project, a topic and Management.
 */
import type { Org } from '@/generated/api-types';
import { Trail } from '@/features/trace/Trail';
import { scopeOf } from '@/features/trace/params';
export function Audit({ org }: { org: Org }) { return <Trail scope={scopeOf(org)} />; }
