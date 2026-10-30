/** Media viewers ask for the same bounded preview and effective byte policy before decoding an original. */
import { useQuery } from '@tanstack/react-query';
import type { Door } from '@/api/workspace';
import type { FileHead } from './content';
import { previewQuery, useFileSource, workspaceSource } from './source';

export function useMediaPreview(door: Door, path: string, head: FileHead) {
  const attached = useFileSource();
  const hash = head.brought.state === 'ready' ? head.brought.hash : null;
  const source = attached !== null && attached.workspace === undefined ? attached : {
    ...workspaceSource(door, path, head.stat.name, head.stat.size),
    url: `/environments/${door.environmentId}/files/content?${new URLSearchParams({ path, hash: hash ?? '', preview: 'true', ...(door.threadId === null ? {} : { thread: door.threadId }) })}`,
  };
  return useQuery({ ...previewQuery(source), enabled: hash !== null });
}
