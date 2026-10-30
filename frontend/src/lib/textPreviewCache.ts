/** A bounded cache of private paste previews; evicted text is read from its scoped blob. */

// Four MiB holds several inline drafts; the entry bound also covers empty and tiny previews.
export const TEXT_PREVIEW_CACHE_BYTES = 4 * 1024 * 1024;
export const TEXT_PREVIEW_CACHE_ENTRIES = 64;

export class TextPreviewCache {
  private readonly texts = new Map<string, string>();
  bytes = 0;
  get size(): number { return this.texts.size; }

  set(key: string, text: string): void {
    this.delete(key);
    const bytes = text.length * 2;
    if (bytes > TEXT_PREVIEW_CACHE_BYTES) return;
    while (this.bytes + bytes > TEXT_PREVIEW_CACHE_BYTES || this.texts.size >= TEXT_PREVIEW_CACHE_ENTRIES) {
      const oldest = this.texts.keys().next();
      if (oldest.done) break;
      this.delete(oldest.value);
    }
    this.texts.set(key, text); this.bytes += bytes;
  }

  get(key: string): string | undefined {
    const text = this.texts.get(key);
    if (text !== undefined) { this.texts.delete(key); this.texts.set(key, text); }
    return text;
  }

  delete(key: string): void {
    const text = this.texts.get(key);
    if (text === undefined) return;
    this.bytes -= text.length * 2; this.texts.delete(key);
  }

  clear(): void { this.texts.clear(); this.bytes = 0; }
}
