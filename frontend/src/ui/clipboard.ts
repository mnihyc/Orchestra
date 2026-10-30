/** Copying a text to the clipboard: whether the browser let the page, and the one way that is told to the person.
 *
 * Anything that copies goes through `copyText`: it says "Copied" (or what the caller says was copied, "Link copied")
 * when the text is on the clipboard, and that the browser would not let the page when it is not, so that a copy
 * never fails in silence and no two screens say it in two ways.
 */
import { i18n } from '@/i18n';
import { toast } from '@/ui/Toast';

/** Writes a text to the clipboard. Whether the browser allowed it. */
export async function writeToClipboard(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

/** Copies a text and tells the person. `said` is what is said of success ("Copied" when it is left out). Whether it was copied. */
export async function copyText(text: string, said?: string): Promise<boolean> {
  const done = await writeToClipboard(text);
  toast(done ? { tone: 'success', title: said ?? i18n.t('common.copied') } : { tone: 'danger', title: i18n.t('common.copyRefused') });
  return done;
}
