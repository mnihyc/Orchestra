/** Folder imports keep distinct files and reject paths that cannot be represented safely. */
import { describe, expect, it } from 'vitest';
import { importSource } from '@/features/people/importSource';

function sourceFile(path: string, contents: string): File {
  const file = new File([contents], path.split('/').at(-1) ?? path);
  Object.defineProperty(file, 'webkitRelativePath', { value: path });
  return file;
}

describe('an import source', () => {
  it('keeps folder paths and assets even when files have the same basename', () => {
    const overview = sourceFile('Notes/overview.md', 'See the image');
    const image = sourceFile('Notes/images/diagram.png', 'image');
    const other = sourceFile('Notes/archive/overview.md', 'Another');
    const source = importSource([overview, image, other], false);
    expect(source.fault).toBeNull();
    expect(source.files.map((entry) => [entry.path, entry.file])).toEqual([
      ['Notes/overview.md', overview], ['Notes/images/diagram.png', image], ['Notes/archive/overview.md', other],
    ]);
    expect(source.bytes).toBe(overview.size + image.size + other.size);
  });

  it('refuses duplicate paths instead of silently choosing one file', () => {
    expect(importSource([sourceFile('Notes/page.md', 'one'), sourceFile('Notes/page.md', 'two')], false)).toMatchObject({ fault: 'duplicate-path', path: 'Notes/page.md' });
  });

  it('refuses paths outside the chosen source', () => {
    for (const path of ['/outside.md', '../outside.md', 'Notes/../outside.md', 'Notes\\outside.md', 'Notes//file.md', 'Notes/\nfile.md']) {
      expect(importSource([sourceFile(path, '')], false).fault).toBe('invalid-path');
    }
  });

  it('accepts a single archive for server inspection and refuses several', () => {
    const archive = sourceFile('backup.zip', 'archive');
    expect(importSource([archive], true)).toMatchObject({ fault: null, files: [{ path: 'backup.zip' }] });
    expect(importSource([archive, sourceFile('other.zip', 'archive')], true).fault).toBe('several-archives');
    expect(importSource([], true).fault).toBe('empty');
  });
});
