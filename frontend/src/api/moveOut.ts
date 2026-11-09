/** Atomic cross-org moves consume the native deletion footprint, never two browser writes. */
import { traceJson } from '@/api/trace';
import type { ChatDuplicateAudience, DuplicateBody, DuplicateDestination } from '@/api/duplicates';
import { duplicateObjects } from '@/api/duplicates';
import type { HomeMoveKind, HomeMoveTarget } from '@/features/duplicate/move';
export type MoveOutEffect = 'remove_source' | 'stop_bound_work' | 'end_group_and_history' | 'keep_registration';
export interface MoveOutDeletePlan { subject: { kind: HomeMoveKind; id: string }; destination: DuplicateDestination; revision: number; stamp: string; effects: MoveOutEffect[] }
export type MoveOutCopy = DuplicateBody & { machine_id?: string | null };
export interface ChatMoveOutPlan { delete_plan: MoveOutDeletePlan; notification: ChatDuplicateAudience }
export type MoveOutReview = MoveOutDeletePlan | ChatMoveOutPlan;
export interface MoveOutBody { copy: MoveOutCopy; delete_plan: { revision: number; stamp: string }; notify?: boolean | null; notify_preview_id?: string | null }
export interface MoveOutReceipt { kind: HomeMoveKind; id: string }
const kinds: ReadonlySet<HomeMoveKind> = new Set(['host', 'machine', 'agent', 'skill', 'mount', 'template', 'mcp_server', 'chat', 'provider']);
export function supportsMoveOut(kind: HomeMoveKind): boolean { return kinds.has(kind); }
export function moveOutPath(target: Pick<HomeMoveTarget, 'kind' | 'id' | 'draft'>): string {
  if (!supportsMoveOut(target.kind)) throw new Error('This family has no published native move route.');
  const object = target.kind === 'host' ? 'hosts' : target.kind === 'machine' ? 'machines' : duplicateObjects[target.kind];
  return `/${object}/${encodeURIComponent(target.id)}${target.kind === 'skill' && target.draft === true ? '/draft' : ''}/move-out`;
}
const mcpCopy = (copy: MoveOutCopy) => ({ name: copy.name ?? null, destination: copy.destination ?? null, expected_revision: copy.expected_revision ?? null, machine_id: copy.machine_id ?? null });
export async function previewMoveOut(target: HomeMoveTarget, copy: MoveOutCopy, signal?: AbortSignal): Promise<MoveOutReview> {
  const review = await traceJson<MoveOutReview>(`${moveOutPath(target)}/preview`, { method: 'POST', body: target.kind === 'mcp_server' ? mcpCopy(copy) : copy, ...(signal === undefined ? {} : { signal }) });
  if ((target.kind === 'chat') !== ('delete_plan' in review)) throw new Error('The deletion preview used another native family.');
  const plan = 'delete_plan' in review ? review.delete_plan : review;
  if (plan.subject.kind !== target.kind || plan.subject.id !== target.id || (plan.destination.kind !== copy.destination?.kind || plan.destination.kind === 'node' && (copy.destination.kind !== 'node' || plan.destination.id !== copy.destination.id)) || plan.stamp === '') throw new Error('The deletion preview did not match its source and destination.');
  return review;
}
export async function executeMoveOut(target: HomeMoveTarget, body: MoveOutBody, key: string): Promise<MoveOutReceipt> {
  const wire = target.kind === 'mcp_server' ? { copy: mcpCopy(body.copy), delete_plan: body.delete_plan } : target.kind === 'chat' ? { ...body, notify: body.notify ?? null, notify_preview_id: body.notify_preview_id ?? null } : body;
  const receipt = await traceJson<unknown>(moveOutPath(target), { method: 'POST', body: wire, key });
  if (typeof receipt !== 'object' || receipt === null || !('projection' in receipt) || receipt.projection !== 'receipt' || !('kind' in receipt) || receipt.kind !== target.kind) throw new Error('The move acknowledgement did not identify its native family.');
  const id = 'id' in receipt ? receipt.id : 'object_id' in receipt ? receipt.object_id : undefined;
  if (typeof id !== 'string' || id === '' || (target.kind === 'host' || target.kind === 'machine') && id !== target.id) throw new Error('The move acknowledgement did not identify the moved resource.');
  if ((target.kind === 'host' || target.kind === 'machine') && (body.copy.destination?.kind !== 'node' || !('home_id' in receipt) || receipt.home_id !== body.copy.destination.id || !('changed' in receipt) || typeof receipt.changed !== 'boolean')) throw new Error('The move acknowledgement did not identify the accepted destination.');
  return { kind: target.kind, id };
}
