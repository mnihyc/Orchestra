/** A source selection accepts folder paths and files without mistaking an internal reference for an upload. */
import { fireEvent, screen } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, expect, it } from 'vitest';
import { ImportSourceStep } from '@/features/people/ImportSourceStep';
import type { ImportSource } from '@/features/people/importSource';
import { setLocale } from '@/i18n';
import { ORCH_REFERENCE } from '@/lib/drop';
import { show } from './support';

function Source() {
  const [value, setValue] = useState<ImportSource | null>(null);
  return <ImportSourceStep value={value} onChange={setValue} />;
}

function folderFile(path: string, text: string): File {
  const file = new File([text], path.split('/').at(-1) ?? path);
  Object.defineProperty(file, 'webkitRelativePath', { value: path });
  return file;
}

afterEach(async () => { await setLocale('en'); });

it('offers the three ways to choose a source and preserves folder paths with assets', () => {
  show(<Source />);
  expect(screen.getByRole('button', { name: 'Choose files' })).toBeEnabled();
  expect(screen.getByRole('button', { name: 'Choose a folder' })).toBeEnabled();
  expect(screen.getByRole('button', { name: 'Choose an archive' })).toBeEnabled();
  const input = screen.getByTestId('import-folder');
  expect(input).toHaveAttribute('webkitdirectory');
  fireEvent.change(input, { target: { files: [folderFile('Notes/page.md', 'text'), folderFile('Notes/images/map.png', 'image')] } });
  expect(screen.getByText('Notes/page.md')).toBeVisible();
  expect(screen.getByText('Notes/images/map.png')).toBeVisible();
  expect(screen.getByText(/^2 files ·/)).toBeVisible();
});

it('accepts dropped files but leaves internal references untouched', async () => {
  show(<Source />);
  const zone = screen.getByTestId('import-drop-zone');
  fireEvent.drop(zone, { dataTransfer: { types: [ORCH_REFERENCE], files: [], items: [] } });
  expect(screen.queryByRole('list', { name: 'Selected files' })).not.toBeInTheDocument();
  const picked = new File(['one'], 'plan.md');
  fireEvent.drop(zone, { dataTransfer: { types: ['Files'], files: [picked], items: [] } });
  expect(await screen.findByText('plan.md')).toBeVisible();
  expect(screen.getByText(/^1 file ·/)).toBeVisible();
});

it('shows more files without imposing a selection limit', () => {
  show(<Source />);
  const files = Array.from({ length: 52 }, (_, index) => new File(['x'], `page-${index}.md`));
  fireEvent.change(screen.getByTestId('people-import-files'), { target: { files } });
  expect(screen.queryByText('page-51.md')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Next page' }));
  expect(screen.getByText('page-51.md')).toBeVisible();
  expect(screen.getByText(/^52 files ·/)).toBeVisible();
  expect(screen.queryByText('page-0.md')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Back to first page' }));
  expect(screen.getByText('page-0.md')).toBeVisible();
});

it('names duplicate source paths in Chinese without silently discarding a file', async () => {
  await setLocale('zh-CN');
  show(<Source />);
  fireEvent.change(screen.getByTestId('import-folder'), { target: { files: [folderFile('Notes/page.md', 'one'), folderFile('Notes/page.md', 'two')] } });
  expect(screen.getByRole('alert')).toHaveTextContent('两个所选文件的路径相同');
  expect(screen.queryByRole('list', { name: '所选文件' })).not.toBeInTheDocument();
});
