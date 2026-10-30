/** Saving a file the page holds: under the name the server gave it, through the browser's own download.
 *
 * The files of an export are read into the page, because the access token is kept in memory and a plain
 * link would carry none. They are handed to the browser as a link to the bytes, which is followed once and let
 * go of a moment later.
 */

/** How long the link to the bytes is kept after it was followed: long enough for the browser to have begun. */
const LET_GO_AFTER_MS = 10_000;

/** Hands a file to the browser to save. */
export function saveFile(blob: Blob, name: string): void {
  const address = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = address;
  link.download = name;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(address), LET_GO_AFTER_MS);
}
