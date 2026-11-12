/** The images and files a message carries: thumbnails that open larger, and chips with a name and a size that download.
 * Every chat attachment uses the same bounded preview, card and workspace viewer as other file surfaces.
 */
import { linkTo } from '@/lib/links';
import { FileAttachment } from '@/features/files/FileChip';
import { blobSource } from '@/features/files/source';
import type { Author } from '@/generated/api-types';

export interface ImageProps { chatId: string; messageId?: string; blob: string; name: string; mediaType?: string; owner?: Author | undefined; updatedAt?: string | undefined }
export function ImageThumb({ chatId, messageId, blob, name, mediaType, owner, updatedAt }: ImageProps) {
  return <FileAttachment source={{ ...blobSource('chat', chatId, blob, name, undefined, mediaType), href: linkTo({ kind: 'chat', id: chatId }, messageId === undefined ? {} : { message: messageId }), owner, updatedAt }} />;
}
export interface FileChipProps extends ImageProps { size: number }
export function FileChip({ chatId, messageId, blob, name, size, mediaType, owner, updatedAt }: FileChipProps) {
  return <FileAttachment source={{ ...blobSource('chat', chatId, blob, name, size, mediaType), href: linkTo({ kind: 'chat', id: chatId }, messageId === undefined ? {} : { message: messageId }), owner, updatedAt }} />;
}
