/** Search and Inbox attachment projections use the same file chip without a second preview implementation. */
import { FileAttachment } from './FileChip';
import { blobSource, documentSource, sourcesOfParts, type FileSource } from './source';
import type { Notification, SearchHit } from '@/generated/api-types';
import { i18n } from '@/i18n';

export function MessageFiles({ files }: { files: readonly FileSource[] }) {
  return files.length === 0 ? null : <span className="flex flex-wrap gap-1" data-message-files="">
    {files.map((source, index) => <FileAttachment key={`${source.url}:${index}`} source={source} presentation={{ place: 'compact' }} thumbnail={false} />)}
  </span>;
}

function imageName(name: string | null | undefined): string {
  const title = name?.trim();
  return title === undefined || title === '' ? i18n.t('timeline.step.imageUnnamed') : title;
}

export function filesOfHit(hit: SearchHit): FileSource[] {
  return hit.kind === 'chat_message' ? sourcesOfParts('chat', hit.chat.id, hit.message.parts)
    : hit.kind === 'item' && hit.item.kind === 'user_message' ? sourcesOfParts('thread', hit.thread.id, hit.item.payload.parts)
    : hit.kind === 'item' && hit.item.kind === 'tool_call' && hit.item.payload.tool.kind === 'image' ? [blobSource('thread', hit.thread.id, hit.item.payload.tool.blob, imageName(hit.item.payload.tool.name), undefined, hit.item.payload.tool.media_type)]
    : hit.kind === 'document' && hit.file !== undefined && hit.file !== null ? [documentSource(hit.document.id, hit.file)] : [];
}

export function SearchFiles({ hit }: { hit: SearchHit }) {
  return <MessageFiles files={filesOfHit(hit)} />;
}

export function notificationFiles(row: Notification): FileSource[] {
  if (row.chat_id === null || typeof row.data !== 'object' || row.data === null || !('files' in row.data) || !Array.isArray(row.data.files)) return [];
  return row.data.files.flatMap((file: unknown) => {
    if (typeof file !== 'object' || file === null || !('blob' in file) || !('name' in file) || !('media_type' in file)
      || typeof file.blob !== 'string' || !/^[a-f\d]{64}$/.test(file.blob) || typeof file.name !== 'string' || typeof file.media_type !== 'string') return [];
    const size = 'size' in file && typeof file.size === 'number' && Number.isSafeInteger(file.size) && file.size >= 0 ? file.size : undefined;
    return [blobSource('chat', row.chat_id ?? '', file.blob, file.name, size, file.media_type)];
  });
}

export function NotificationFiles({ row }: { row: Notification }) {
  return <MessageFiles files={notificationFiles(row)} />;
}
