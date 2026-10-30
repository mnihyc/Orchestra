/** Import selections are several Markdown/HTML files or one archive, with stable entry identities. */

export type ImportFormat = 'markdown' | 'html' | 'archive';
/** The existing archive listing bound, shared with the server's import guard. */
export const IMPORT_ENTRIES_MAX = 200;
export function formatOf(name: string): ImportFormat | null {
  return /\.(md|markdown|txt)$/i.test(name) ? 'markdown' : /\.html?$/i.test(name) ? 'html' : /\.zip$/i.test(name) ? 'archive' : null;
}
export function selectionFault(files: readonly Pick<File, 'name'>[]): 'mixed' | 'unsupported' | null {
  if (files.some((file) => formatOf(file.name) === null)) return 'unsupported';
  return files.length > 1 && files.some((file) => formatOf(file.name) === 'archive') ? 'mixed' : null;
}
