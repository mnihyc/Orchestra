// @vitest-environment node
/** The Typst bundle keeps images beside their source, refuses escaping paths and expansion beyond its memory allowance, and frees a failed compiler project. */
import { describe, expect, it, vi } from 'vitest';

import { typesetPdf } from '@/features/documents/pdf';
import { readTypstBundle } from '@/features/documents/typstBundle';
import { MEDIA_MOST_BYTES } from '@/features/files/viewers';

import { zipFilesOf } from '../doubles/export.ts';

const compiler = vi.hoisted(() => {
  const files: Record<string, string | Uint8Array> = {};
  return {
  diagnostics: [] as { severity: 'error' | 'warning'; message: string }[],
  files,
  destroyed: vi.fn(),
  fonts: vi.fn(),
  };
});
vi.mock('@/api/client', () => ({ assetBytes: () => Promise.resolve(new Uint8Array([1, 2, 3])) }));
vi.mock('@vedivad/typst-web-service', () => ({ TypstProject: {
  create: () => Promise.resolve({
    addFont: compiler.fonts,
    setMany: (files: Record<string, string | Uint8Array>) => { compiler.files = files; return Promise.resolve(); },
    compile: () => Promise.resolve({ diagnostics: compiler.diagnostics, pages: [] }),
    exportPdf: () => Promise.resolve(new TextEncoder().encode('%PDF-1.7\nfixture')),
    destroy: compiler.destroyed,
  }),
} }));

const encoded = (text: string) => new TextEncoder().encode(text);
const bundle = (entries: [string, Uint8Array][], deflated = false) => new Blob([zipFilesOf(entries, deflated).slice()], { type: 'application/zip' });

describe('a local PDF export', () => {
  it.each([false, true])('reads the source and its exact image bytes from stored/deflated entries (%s)', async (deflated) => {
    const image = new Uint8Array([0, 17, 255, 0, 9]);
    const files = await readTypstBundle(bundle([['main.typ', encoded('= 你好')], ['images/图.png', image]], deflated));
    expect(new TextDecoder().decode(files.get('/main.typ'))).toBe('= 你好');
    expect(files.get('/images/图.png')).toEqual(image);
    expect(files.size).toBe(2);
  });

  it('refuses paths that escape the virtual document and repeated paths instead of replacing image bytes', async () => {
    for (const name of ['../main.typ', '/main.typ', 'images/../a.png', 'a\\b.png']) {
      await expect(readTypstBundle(bundle([['main.typ', encoded('text')], [name, encoded('bad')]]))).rejects.toThrow();
    }
    await expect(readTypstBundle(bundle([['main.typ', encoded('one')], ['main.typ', encoded('two')]]))).rejects.toThrow();
  });

  it('refuses a declared expansion beyond the preview allowance before allocating it', async () => {
    const bytes = zipFilesOf([['main.typ', encoded('text')]]).slice();
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const central = view.getUint32(bytes.length - 22 + 16, true);
    view.setUint32(central + 24, MEDIA_MOST_BYTES + 1, true);
    await expect(readTypstBundle(new Blob([bytes]))).rejects.toThrow(/too large/);
  });

  it('passes authorized images to a private project, adds local fonts and tears the worker down on success and diagnostics', async () => {
    compiler.destroyed.mockClear(); compiler.fonts.mockClear(); compiler.diagnostics = [];
    const original = bundle([['main.typ', encoded('= Unicode 你好')], ['images/chart.png', new Uint8Array([1, 9])]]);
    const file = await typesetPdf(original);
    expect(file.blob.type).toBe('application/pdf');
    expect(compiler.files['/images/chart.png']).toEqual(new Uint8Array([1, 9]));
    expect(new TextDecoder().decode(compiler.files['/main.typ'] as Uint8Array)).toContain('= Unicode 你好');
    expect(compiler.fonts).toHaveBeenCalledTimes(2);
    expect(compiler.destroyed).toHaveBeenCalledTimes(1);
    compiler.diagnostics = [{ severity: 'error', message: 'A formula could not be typeset' }];
    await expect(typesetPdf(original)).rejects.toThrow('A formula could not be typeset');
    expect(compiler.destroyed).toHaveBeenCalledTimes(2);
  });
});
