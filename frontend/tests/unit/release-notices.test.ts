/** Bundle inputs retain nested npm identity without leaking the build machine's paths. */
import { describe, expect, it } from 'vitest';
import { npmSource } from '../../build/release-notices';

describe('release notice inputs', () => {
  it('retains asset paths, scoped names and nested package versions', () => {
    expect(npmSource('/build/frontend/node_modules/@scope/wrapper/node_modules/font/fonts/A.ttf?url'))
      .toEqual({ path: 'node_modules/@scope/wrapper/node_modules/font', source: 'fonts/A.ttf' });
    expect(npmSource('C:\\build\\node_modules\\pdfjs-dist\\wasm\\decoder.wasm'))
      .toEqual({ path: 'node_modules/pdfjs-dist', source: 'wasm/decoder.wasm' });
    expect(npmSource('/build/frontend/src/app.ts')).toBeUndefined();
  });

});
