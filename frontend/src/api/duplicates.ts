/** Native copy operations accept source identities, never reconstructed configuration or credentials. */
import { accepted, api, repeatable } from '@/api/client';
import { copySetupNotes, type CopySetupNote } from '@/api/copySetup';
import type {
  ChatDuplicateAudience, ChatDuplicateBody, DocumentDirectory, DocumentTreeOperationBody, DocumentTreeOperationResponse,
  DuplicateBody as NativeDuplicateBody, DuplicateKind, DuplicateReceipt as NativeDuplicateReceipt,
  EnvironmentDuplicateBody, EnvironmentDuplicateManaged, McpDuplicateBody,
} from '@/generated/api-types';
export type { ChatDuplicateAudience, DuplicateDestination, DuplicateKind } from '@/generated/api-types';
export type DuplicateReceipt = NativeDuplicateReceipt & { setup?: CopySetupNote[] };

/** UI omission means the native field's None/default; generated wire fields are nullable. */
export type DuplicateBody = Partial<NativeDuplicateBody>;
export interface DuplicateOptions { draft?: boolean; machineId?: string; notify?: { preview: string }; managed?: Partial<EnvironmentDuplicateManaged> }
export const duplicateObjects: Record<DuplicateKind, string> = {
  provider: 'providers', agent: 'agents', template: 'templates', mcp_server: 'mcp-servers',
  mount: 'mounts', skill: 'skills', environment: 'environments', chat: 'chats',
};
export function duplicatePath(kind: DuplicateKind, id: string, draft = false): string {
  return `/${duplicateObjects[kind]}/${encodeURIComponent(id)}${kind === 'skill' && draft ? '/draft' : ''}/duplicate`;
}
const ordinary = (body: DuplicateBody): NativeDuplicateBody => ({ name: body.name ?? null, destination: body.destination ?? null, expected_revision: body.expected_revision ?? null });
const mcp = (body: DuplicateBody, options: DuplicateOptions): McpDuplicateBody => ({ ...ordinary(body), machine_id: options.machineId ?? null });
const environment = (body: DuplicateBody, options: DuplicateOptions): EnvironmentDuplicateBody => ({ ...ordinary(body), managed: options.managed === undefined ? null : { template_id: options.managed.template_id ?? null, host_id: options.managed.host_id ?? null }, machine_id: options.machineId ?? null });
const chat = (body: DuplicateBody, options: DuplicateOptions): ChatDuplicateBody => ({ copy: ordinary(body), notify: options.notify === undefined ? null : true, notify_preview_id: options.notify?.preview ?? null });
export function duplicateBody(kind: DuplicateKind, body: DuplicateBody, options: DuplicateOptions = {}): object {
  switch (kind) { case 'chat': return chat(body, options); case 'environment': return environment(body, options); case 'mcp_server': return mcp(body, options); default: return ordinary(body); }
}
function sendDuplicate(kind: DuplicateKind, id: string, body: DuplicateBody, key: string, options: DuplicateOptions): Promise<NativeDuplicateReceipt> {
  const request = { params: { path: { id } }, ...repeatable(key) };
  switch (kind) {
    case 'provider': return accepted(api.POST('/providers/{id}/duplicate', { ...request, body: ordinary(body) }));
    case 'agent': return accepted(api.POST('/agents/{id}/duplicate', { ...request, body: ordinary(body) }));
    case 'template': return accepted(api.POST('/templates/{id}/duplicate', { ...request, body: ordinary(body) }));
    case 'mount': return accepted(api.POST('/mounts/{id}/duplicate', { ...request, body: ordinary(body) }));
    case 'mcp_server': return accepted(api.POST('/mcp-servers/{id}/duplicate', { ...request, body: mcp(body, options) }));
    case 'environment': return accepted(api.POST('/environments/{id}/duplicate', { ...request, body: environment(body, options) }));
    case 'chat': return accepted(api.POST('/chats/{id}/duplicate', { ...request, body: chat(body, options) }));
    case 'skill': return options.draft === true ? accepted(api.POST('/skills/{id}/draft/duplicate', { ...request, body: ordinary(body) })) : accepted(api.POST('/skills/{id}/duplicate', { ...request, body: ordinary(body) }));
  }
}
export async function duplicate(kind: DuplicateKind, id: string, body: DuplicateBody, key: string, options: DuplicateOptions = {}): Promise<DuplicateReceipt> {
  const receipt: unknown = await sendDuplicate(kind, id, body, key, options);
  if (typeof receipt !== 'object' || receipt === null || !('projection' in receipt) || receipt.projection !== 'receipt' || !('kind' in receipt) || receipt.kind !== kind || !('id' in receipt) || typeof receipt.id !== 'string' || receipt.id === '') throw new Error('The copy acknowledgement did not identify the new resource.');
  const setup = 'setup' in receipt ? copySetupNotes(receipt.setup) : undefined;
  return { projection: receipt.projection, kind, id: receipt.id, ...(setup === undefined ? {} : { setup }) };
}
export const previewChatDuplicate = (id: string, signal?: AbortSignal): Promise<ChatDuplicateAudience> => accepted(api.POST('/chats/{id}/duplicate/preview', { params: { path: { id } }, ...(signal === undefined ? {} : { signal }) }));

/** Documents retain the native folder-copy writer and its fresh-identity map. */
export async function duplicateDocument(id: string, directory: DocumentDirectory, name: string | undefined, key: string): Promise<{ kind: 'document'; id: string }> {
  const answer = await copyDocumentTree({ command: 'copy', items: [{ kind: 'document', id }], to: directory, name: name ?? null, client_id: key, version: null, deletion_plan: null });
  const item = 'projection' in answer ? answer.entries.find((entry) => entry.kind === 'document') : answer.copied.find((entry) => entry.source.kind === 'document' && entry.source.id === id)?.destination;
  if (item?.kind !== 'document') throw new Error('The copy acknowledgement did not identify the new document.');
  return item;
}
function copyDocumentTree(body: DocumentTreeOperationBody): Promise<DocumentTreeOperationResponse> {
  return accepted(api.POST('/document-tree/operations', { body, ...repeatable(body.client_id) }));
}
