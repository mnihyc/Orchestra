/** Reads Orch's Typst export zip into bounded virtual files: source and authorized images, with no extraction to a machine's filesystem. */
import { MEDIA_MOST_BYTES } from '@/features/files/viewers';
import { i18n } from '@/i18n';

/** The same memory allowance as a media preview, shared across all bundle entries. */
const MOST_BYTES = MEDIA_MOST_BYTES;
const decoder = new TextDecoder('utf-8', { fatal: true });

function invalid(): Error {
  return new Error(i18n.t('documents.pdf.invalidBundle'));
}

/** The server writes ordinary (non-Zip64) stored/deflated entries; the central
 * directory supplies their actual sizes even when local headers have a descriptor. */
export async function readTypstBundle(blob: Blob): Promise<Map<string, Uint8Array>> {
  if (blob.size > MOST_BYTES) throw new Error(i18n.t('documents.pdf.tooBig'));
  const bytes = new Uint8Array(await blob.arrayBuffer());
  const view = new DataView(bytes.buffer);
  const fits = (at: number, size: number) => Number.isSafeInteger(at) && at >= 0 && at + size <= bytes.length;
  let end = bytes.length - 22;
  // A zip's trailing comment is at most one unsigned 16-bit length.
  const least = Math.max(0, end - 65_535);
  for (; end >= least; end--) {
    if (fits(end, 22) && view.getUint32(end, true) === 0x06054b50 && end + 22 + view.getUint16(end + 20, true) === bytes.length) break;
  }
  if (end < least || !fits(end, 22) || view.getUint16(end + 4, true) !== 0 || view.getUint16(end + 6, true) !== 0) throw invalid();
  const count = view.getUint16(end + 10, true);
  const size = view.getUint32(end + 12, true);
  let at = view.getUint32(end + 16, true);
  if (count === 0xffff || size === 0xffffffff || !fits(at, size) || at + size > end) throw invalid();
  const directoryEnd = at + size;
  const files = new Map<string, Uint8Array>();
  let total = 0;
  for (let n = 0; n < count; n++) {
    if (!fits(at, 46) || view.getUint32(at, true) !== 0x02014b50) throw invalid();
    const flags = view.getUint16(at + 8, true);
    const method = view.getUint16(at + 10, true);
    const compressed = view.getUint32(at + 20, true);
    const plain = view.getUint32(at + 24, true);
    const nameSize = view.getUint16(at + 28, true);
    const extraSize = view.getUint16(at + 30, true);
    const commentSize = view.getUint16(at + 32, true);
    const local = view.getUint32(at + 42, true);
    if (flags & 1 || !fits(at + 46, nameSize + extraSize + commentSize) || plain === 0xffffffff || local === 0xffffffff) throw invalid();
    const name = decoder.decode(bytes.subarray(at + 46, at + 46 + nameSize));
    at += 46 + nameSize + extraSize + commentSize;
    if (at > directoryEnd || name.startsWith('/') || name.includes('\\') || name.split('/').some((part) => part === '..' || part === '.') || name.includes('\0')) throw invalid();
    if (name.endsWith('/')) continue;
    if (name === '' || files.has(`/${name}`) || !fits(local, 30) || view.getUint32(local, true) !== 0x04034b50) throw invalid();
    total += plain;
    if (total > MOST_BYTES) throw new Error(i18n.t('documents.pdf.tooBig'));
    const start = local + 30 + view.getUint16(local + 26, true) + view.getUint16(local + 28, true);
    if (!fits(start, compressed) || start + compressed > directoryEnd) throw invalid();
    const stored = bytes.slice(start, start + compressed);
    let content: Uint8Array;
    if (method === 0) content = stored;
    else if (method === 8) {
      const stream = new Blob([stored]).stream().pipeThrough(new DecompressionStream('deflate-raw'));
      const reader = stream.getReader();
      content = new Uint8Array(plain);
      let written = 0;
      try {
        for (;;) {
          const next = await reader.read();
          if (next.done) break;
          if (written + next.value.length > plain) throw invalid();
          content.set(next.value, written);
          written += next.value.length;
        }
        if (written !== plain) throw invalid();
      } finally {
        await reader.cancel();
        reader.releaseLock();
      }
    } else throw invalid();
    if (content.length !== plain) throw invalid();
    files.set(`/${name}`, content);
  }
  if (at !== directoryEnd || !files.has('/main.typ')) throw invalid();
  return files;
}
