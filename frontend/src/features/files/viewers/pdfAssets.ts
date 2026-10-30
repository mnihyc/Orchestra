/** PDF.js's CMaps, standard fonts and decoders are local lazy assets, fetched by filename when a PDF needs them. */
import { assetBytes } from '@/api/client';
import { i18n } from '@/i18n';
const assets = import.meta.glob<string>('/node_modules/pdfjs-dist/{cmaps,standard_fonts,wasm}/*', { query: '?url', import: 'default' });

async function bytesOf(directory: string, name: string): Promise<Uint8Array> {
  const load = assets[`/node_modules/pdfjs-dist/${directory}/${name}`];
  if (load === undefined) throw new Error(i18n.t('files.pdf.assetFailed'));
  return assetBytes(await load());
}

/** PDF.js 6 has one BinaryDataFactory for all three kinds of asset. */
export class LocalBinaryData {
  fetch({ kind, filename }: { kind: 'cMapUrl' | 'standardFontDataUrl' | 'wasmUrl'; filename: string }): Promise<Uint8Array> {
    const directory = { cMapUrl: 'cmaps', standardFontDataUrl: 'standard_fonts', wasmUrl: 'wasm' }[kind];
    return bytesOf(directory, filename);
  }
}
