/** File selections keep their relative paths and cannot silently overwrite another selected item. */

/** One browser file and its path within the selected source. */
export interface ImportSourceFile {
  file: File;
  path: string;
}

/** Problems that can be decided before asking the server to inspect the source. */
export type ImportSourceFault = 'empty' | 'invalid-path' | 'duplicate-path' | 'several-archives';

/** A source ready to upload, or the reason it cannot be used. */
export type ImportSource =
  | { files: ImportSourceFile[]; bytes: number; fault: null }
  | { files: []; bytes: 0; fault: ImportSourceFault; path?: string };

/** The browser's relative name is preserved for links and assets inside a folder. */
function* inspectSource(files: readonly File[], archive: boolean): Generator<void, ImportSource> {
  if (files.length === 0) return { files: [], bytes: 0, fault: 'empty' };
  if (archive && files.length !== 1) return { files: [], bytes: 0, fault: 'several-archives' };
  const paths = new Set<string>();
  const entries: ImportSourceFile[] = [];
  let bytes = 0;
  for (const file of files) {
    const path = file.webkitRelativePath || file.name;
    if (path.startsWith('/') || path.includes('\\') || path.split('/').some((part) => part === '' || part === '.' || part === '..' || /\p{Cc}/u.test(part))) {
      return { files: [], bytes: 0, fault: 'invalid-path', path };
    }
    if (paths.has(path)) return { files: [], bytes: 0, fault: 'duplicate-path', path };
    paths.add(path);
    entries.push({ file, path });
    bytes += file.size;
    if (entries.length % SOURCE_BATCH === 0) yield;
  }
  return { files: entries, bytes, fault: null };
}

/** A short chunk of path checks lets the browser draw progress and handle cancellation between chunks. */
export const SOURCE_BATCH = 512;

/** Small selections can be inspected immediately; this also supplies the pure validation used in tests. */
export function importSource(files: readonly File[], archive: boolean): ImportSource {
  const reading = inspectSource(files, archive);
  let step = reading.next();
  while (!step.done) step = reading.next();
  return step.value;
}

/** Large selections cooperate with the browser, without changing path or duplicate checks. */
export async function readImportSource(files: readonly File[], archive: boolean, signal: AbortSignal,
  pause: () => Promise<void> = () => new Promise((resolve) => setTimeout(resolve, 0))): Promise<ImportSource> {
  const reading = inspectSource(files, archive);
  for (;;) {
    signal.throwIfAborted();
    const step = reading.next();
    if (step.done) return step.value;
    await pause();
  }
}
