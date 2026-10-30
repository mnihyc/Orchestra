/** Paste folding follows the rendered editor; delivery follows the remaining context, not a character threshold. */
import { attachToChat, attachToThread } from '@/lib/uploads';
import { api, client, ok } from '@/api/client';
import type { ChatPart, ContentPart, ContextFill, Thread, UploadedBlob } from '@/generated/api-types';
import { countTextCharactersAsync, estimateTextTokens, estimateTokenCounts } from '@/lib/tokenEstimate';
import { TextPreviewCache } from '@/lib/textPreviewCache';

export type PastedTextPart = Extract<ContentPart, { type: 'pasted_text' }>;
/** A chip keeps its private copy identity while its immutable content version changes. */
export function pasteDisplayKey(paste: PastedTextPart): string { return `${paste.path}\u0000${paste.name}`; }
/** Equal bytes can be attached twice; their private destinations remain separate edits. */
export function pasteKey(paste: PastedTextPart): string { return `${pasteDisplayKey(paste)}\u0000${paste.blob}`; }
/** Mirrors the server's shipped setting while its inherited value loads. */
export const DEFAULT_PASTE_INLINE_CONTEXT_SHARE = 25;

// The server's inline-input budget, conservatively measured in UTF-16 units here.
export const INLINE_PASTE_MAX_CHARS = 100_000;
const localTexts = new TextPreviewCache();
client.onSession((event) => { if (event.type === 'signed_in' || event.type === 'signed_out') localTexts.clear(); });
client.session.subscribe((next, previous) => { if (next.readerVersion !== previous.readerVersion) localTexts.clear(); });
export function rememberPaste(blob: string, text: string): void { localTexts.set(blob, text); }
export function rememberedPaste(blob: string): string | undefined { return localTexts.get(blob); }
export function forgetPaste(blob: string): void { localTexts.delete(blob); }

/** Uses the selected model's reported prose estimate, as the server does for a paste. */
export const estimatedPasteTokens = estimateTextTokens;

export function pastedPartTokens(paste: PastedTextPart, estimate: unknown): number | null {
  return estimateTokenCounts(paste.ascii_characters, paste.characters - paste.ascii_characters, estimate);
}

type PasteContext = Pick<NonNullable<Thread['usage']>, 'context_window' | 'context_used'> | null | undefined;
/** The selected fill includes Orch's estimates where a harness gives no count. */
export function pasteContext(usage: PasteContext, fill: Pick<ContextFill, 'used' | 'window'> | null | undefined): PasteContext {
  return fill === null || fill === undefined ? usage : { context_used: fill.used, context_window: fill.window ?? usage?.context_window ?? null };
}

export function pastedPartFits(paste: PastedTextPart, usage: PasteContext, estimate: unknown, share: number, window?: number | null, earlierPastes = 0): boolean {
  if (paste.delivery !== undefined && paste.delivery !== null) return paste.delivery === 'inline';
  if (share === 100) return true;
  const reported = usage?.context_window;
  const size = reported !== null && reported !== undefined && window !== null && window !== undefined ? Math.min(reported, window) : reported ?? window;
  const used = usage?.context_used ?? null;
  const tokens = pastedPartTokens(paste, estimate);
  return share > 0 && size !== null && size !== undefined && used !== null && tokens !== null && tokens + earlierPastes <= Math.floor(Math.max(0, size - used) * share / 100);
}

export function pasteFits(text: string, usage: PasteContext, estimate: unknown, share: number, window?: number | null): boolean {
  if (share === 100) return true;
  const reported = usage?.context_window;
  const size = reported !== null && reported !== undefined && window !== null && window !== undefined ? Math.min(reported, window) : reported ?? window;
  const used = usage?.context_used ?? null;
  const tokens = estimatedPasteTokens(text, estimate);
  return share > 0 && size !== null && size !== undefined && used !== null && tokens !== null && tokens <= Math.max(0, size - used) * share / 100;
}

/** Measures wrapping at the editor's actual width and font, including the words already there. */
export function pasteWouldScroll(element: HTMLTextAreaElement, nextText: string, maxLines: number): boolean {
  if (nextText.length > INLINE_PASTE_MAX_CHARS) return true;
  const style = getComputedStyle(element);
  const clone = element.cloneNode(false) as HTMLTextAreaElement;
  clone.removeAttribute('id'); clone.removeAttribute('aria-label'); clone.tabIndex = -1;
  clone.style.cssText = 'position:fixed;left:-10000px;top:0;visibility:hidden;height:0;min-height:0;max-height:none;overflow:hidden;';
  for (const key of ['font', 'line-height', 'letter-spacing', 'word-spacing', 'padding', 'border', 'box-sizing', 'white-space', 'tab-size']) clone.style.setProperty(key, style.getPropertyValue(key));
  clone.style.width = `${element.getBoundingClientRect().width}px`;
  clone.value = nextText; document.body.appendChild(clone);
  const lines = Number.parseFloat(style.lineHeight) || 20;
  const padding = (Number.parseFloat(style.paddingTop) || 0) + (Number.parseFloat(style.paddingBottom) || 0);
  const cssMaximum = Number.parseFloat(style.maxHeight);
  const maximum = Math.min(lines * maxLines + padding, Number.isFinite(cssMaximum) ? cssMaximum : Number.POSITIVE_INFINITY);
  const folds = clone.scrollHeight > maximum;
  clone.remove();
  return folds;
}

export function pastedFile(text: string): File {
  return new File([text], `pasted-${Date.now()}-${crypto.randomUUID().slice(0, 8)}.txt`, { type: 'text/plain' });
}

export async function attachPastedText(kind: 'thread' | 'chat', id: string, blob: UploadedBlob, text: string, signal: AbortSignal, replace?: string): Promise<PastedTextPart> {
  const counts = await countTextCharactersAsync(text, signal);
  const part: ContentPart | ChatPart = await (kind === 'thread' ? attachToThread(id, blob, signal, replace) : attachToChat(id, blob, signal));
  rememberPaste(blob.blob, text);
  return { type: 'pasted_text', blob: blob.blob, name: part.type === 'file' ? part.name : blob.name, size: blob.size, characters: counts.ascii + counts.other, ascii_characters: counts.ascii, path: part.type === 'file' ? part.path : '' };
}

/** A draft's text stays private; sent text is read through the same scoped blob reader. */
export async function readPastedText(kind: 'thread' | 'chat', id: string, paste: PastedTextPart): Promise<string> {
  const local = rememberedPaste(paste.blob);
  if (local !== undefined) return local;
  const text = kind === 'thread'
    ? await ok(api.GET('/threads/{id}/blobs/{hash}', { params: { path: { id, hash: paste.blob } }, parseAs: 'text' }))
    : await ok(api.GET('/chats/{id}/blobs/{hash}', { params: { path: { id, hash: paste.blob } }, parseAs: 'text' }));
  rememberPaste(paste.blob, text);
  return text;
}
