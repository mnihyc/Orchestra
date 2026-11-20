/** Font files remain same-origin resources under the edge's existing policy. */
import type { PreRenderedAsset } from 'rolldown';

const FONT = /\.(woff2?|ttf|otf)$/i;
export function inlineAsset(file: string): false | undefined { return FONT.test(file) ? false : undefined; }
export function assetFileName(asset: Pick<PreRenderedAsset, 'names'>): string {
  return asset.names.some((name) => FONT.test(name)) ? 'fonts/[name]-[hash][extname]' : 'assets/[name]-[hash][extname]';
}
