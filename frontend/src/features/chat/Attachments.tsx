/** The images and files a message carries: thumbnails that open larger, and chips with a name and a size that download.
 * Every chat attachment uses the same bounded preview, card and workspace viewer as other file surfaces.
 */
import { FileAttachment } from '@/features/files/FileChip';
import { blobSource } from '@/features/files/source';
import type { Author } from '@/generated/api-types';

export interface ImageProps { chatId: string; blob: string; name: string; mediaType?: string; owner?: Author | undefined; updatedAt?: string | undefined }
export function ImageThumb({ chatId, blob, name, mediaType, owner, updatedAt }: ImageProps) {
  return <FileAttachment source={{ ...blobSource('chat', chatId, blob, name, undefined, mediaType), owner, updatedAt }} />;
}
export interface FileChipProps extends ImageProps { size: number }
export function FileChip({ chatId, blob, name, size, mediaType, owner, updatedAt }: FileChipProps) {
  return <FileAttachment source={{ ...blobSource('chat', chatId, blob, name, size, mediaType), owner, updatedAt }} />;
}
