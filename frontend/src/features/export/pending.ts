/** What the export screens ask of a thread, a chat and an archive, now that the generated types name all three.
 *
 * `can.export` on the view of a thread and of a chat says whether the caller may take it away as a file, and an
 * archive's `node` is empty once its place was purged (the generated `Export`, imported where it is used). The two
 * functions are kept as the one place the screens read them from; they are not types and declare nothing; whoever next joins this part reads the members where they are and removes this file.
 */
import type { Chat, Thread } from '@/generated/api-types';
import type { Held } from '@/lib/permissions';
import { threadCapabilities } from '@/features/thread/cached';

/** Whether the caller may take the conversation of a thread away as a file. */
export function mayExportThread(thread: Thread): boolean | null {
  return threadCapabilities(thread)?.export ?? null;
}

/**
 * Whether the caller may take a chat away as a file. The server says it for every kind of chat: a chat of a node
 * by the chat's Export row there, a direct or a group chat by being in it, and not for an admin who only reads it
 * as one. The node's access and the caller are no longer asked; the parameters stay until the callers are joined.
 */
export function mayExportChat(chat: Chat, _node?: Held, _me?: string | null): boolean | null {
  return chat.can.export;
}

