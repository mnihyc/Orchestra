/** Files and references entering an editor keep their origin and never turn an internal drag into a copy. */
import { environmentFileReference, referenceOfAddress } from '@/features/people/references';

export const ORCH_REFERENCE = 'application/x-orch-reference';
export const ORCH_PATHS = 'application/x-orch-paths';

export function carriesFiles(data: DataTransfer | null): boolean {
  return data !== null && Array.from(data.types).includes('Files');
}

/** Drag data exposes item counts before the drop, while file names remain protected. */
export function draggedFileCount(data: Pick<DataTransfer, 'files'> & Partial<Pick<DataTransfer, 'items'>>): number | null {
  const count = Array.from(data.items ?? []).filter((item) => item.kind === 'file').length || data.files.length;
  return count > 0 ? count : null;
}

interface ViewerDragEvent { dataTransfer: DataTransfer | null; preventDefault: () => void; stopPropagation: () => void }

/** Consume a file drop only to prevent the browser from navigating away from unsaved work. */
export function viewerDragOver(event: ViewerDragEvent): void {
  if (event.dataTransfer === null || !carriesFiles(event.dataTransfer) && !carriesReference(event.dataTransfer)) return;
  event.preventDefault(); event.dataTransfer.dropEffect = 'none';
}

export function viewerDrop(event: ViewerDragEvent): void {
  if (event.dataTransfer === null || !carriesFiles(event.dataTransfer) && !carriesReference(event.dataTransfer)) return;
  event.preventDefault(); event.stopPropagation();
}

/** An unclaimed file drop leaves the application in place; native file inputs keep their own behavior. */
export function unclaimedFileDrop(event: DragEvent): void {
  if (!carriesFiles(event.dataTransfer) || event.defaultPrevented || event.target instanceof HTMLInputElement && event.target.type === 'file') return;
  event.preventDefault();
  if (event.dataTransfer !== null) event.dataTransfer.dropEffect = 'none';
}

/** Browser drags of Orch links are references too; ordinary text remains the editor's own paste. */
export function carriesReference(data: DataTransfer): boolean {
  return Array.from(data.types).some((type) => type === ORCH_REFERENCE || type === ORCH_PATHS);
}

/** A reference in the public written grammar, without fetching or uploading its bytes. */
export function droppedReference(data: DataTransfer): string | null {
  const explicit = data.getData(ORCH_REFERENCE);
  if (explicit !== '') return explicit;
  const paths = data.getData(ORCH_PATHS);
  if (paths !== '') {
    try {
      const value: unknown = JSON.parse(paths);
      if (typeof value === 'object' && value !== null && 'environmentId' in value && typeof value.environmentId === 'string'
        && 'paths' in value && Array.isArray(value.paths) && value.paths.every((path): path is string => typeof path === 'string')) {
        const environmentId = value.environmentId;
        return value.paths.map((path) => environmentFileReference(environmentId, path)).join('\n');
      }
    } catch { return null; }
  }
  const uri = data.getData('text/uri-list').split('\n').find((line) => line.trim() !== '' && !line.startsWith('#'));
  if (uri === undefined) return null;
  try {
    const url = new URL(uri, window.location.origin);
    if (url.origin !== window.location.origin) return null;
    const ref = referenceOfAddress(`${url.pathname}${url.hash}`);
    if (ref === null) return null;
    if (ref.kind === 'environment_file' && ref.path !== undefined) return environmentFileReference(ref.id, ref.path, ref.line);
    const part = ref.line ?? ref.turn ?? ref.message;
    return `[${ref.id}](id:${ref.id}${part === undefined ? '' : `#${part}`})`;
  } catch { return null; }
}

/** Inserts at the current selection, retaining words on both sides and returning the new caret. */
export function insertAtSelection(text: string, words: string, element: HTMLTextAreaElement | null): { text: string; caret: number } {
  const start = element?.selectionStart ?? text.length;
  const end = element?.selectionEnd ?? start;
  return { text: `${text.slice(0, start)}${words}${text.slice(end)}`, caret: start + words.length };
}

interface Entry {
  isFile: boolean;
  isDirectory: boolean;
  name: string;
  file: (done: (file: File) => void, fail: (error: DOMException) => void) => void;
  createReader: () => { readEntries: (done: (entries: Entry[]) => void, fail: (error: DOMException) => void) => void };
}

/** Folder drops are enumerated while the drag's entries are still available; relative names survive. */
export async function droppedFiles(data: Pick<DataTransfer, 'files'> & Partial<Pick<DataTransfer, 'items'>>, signal?: AbortSignal, progress?: (files: number, bytes: number) => void): Promise<File[]> {
  signal?.throwIfAborted();
  const items = Array.from(data.items ?? []);
  const entries = items.map((item: Partial<Pick<DataTransferItem, 'webkitGetAsEntry'>>) => item.webkitGetAsEntry?.() as Entry | null | undefined);
  if (!entries.some((entry) => entry?.isDirectory === true)) return Array.from(data.files);
  const result: File[] = [];
  let bytes = 0;
  const visit = async (entry: Entry, parent: string): Promise<void> => {
    signal?.throwIfAborted();
    const path = parent === '' ? entry.name : `${parent}/${entry.name}`;
    if (entry.isFile) {
      const file = await new Promise<File>((resolve, reject) => entry.file(resolve, reject));
      signal?.throwIfAborted();
      Object.defineProperty(file, 'webkitRelativePath', { value: path });
      result.push(file);
      bytes += file.size; progress?.(result.length, bytes);
    } else if (entry.isDirectory) {
      const reader = entry.createReader();
      for (;;) {
        signal?.throwIfAborted();
        const children = await new Promise<Entry[]>((resolve, reject) => reader.readEntries(resolve, reject));
        if (children.length === 0) break;
        for (const child of children) await visit(child, path);
      }
    }
  };
  for (const [index, entry] of entries.entries()) {
    if (entry !== null && entry !== undefined) await visit(entry, '');
    else { const file = items[index]?.getAsFile(); if (file !== null && file !== undefined) { signal?.throwIfAborted(); result.push(file); bytes += file.size; progress?.(result.length, bytes); } }
  }
  return result;
}
