/** Trace types the generated contract does not name, and the trace's own transport for what is not typed yet; every served type is imported from the generated contract where it is used. */
import type { DeletedNode, DocumentRef, UserRef } from '@/generated/api-types';

/** Not generated: a deleted node of a place's list with the current guards its screen offers, which the generated row does not carry. */
export type KeptNode = DeletedNode & { can_inspect?: boolean; can_restore?: boolean };
/** Not generated: a document of a place's deleted list as its screen shows it, with its deletion facts and guards; the generated list row is shaped otherwise. */
export type KeptDocumentRef = DocumentRef & { deletion_id?: string | null; deleted_at?: string; deleted_by?: UserRef | null; purge_at?: string; can_inspect?: boolean; can_restore?: boolean };
/** Not a contract type: the existing client's transport for the requests of `api/trace.ts` that are not typed yet, without a second bearer or fetch implementation. */
export interface PendingTransport {
  request: (method: string, path: string, init: Record<string, unknown>) => Promise<{ data?: unknown; error?: unknown; response: Response }>;
}
