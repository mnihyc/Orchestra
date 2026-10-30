/** A drop retains its destination and conflicts never overwrite without an explicit batch choice. */
import { describe, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import { numberedPath, uploadPath, writeUpload } from '@/features/files/Uploads';
import type { FileStat } from '@/generated/api-types';
import { carriesFiles, draggedFileCount, droppedFiles, droppedReference, insertAtSelection, ORCH_PATHS, ORCH_REFERENCE, unclaimedFileDrop, viewerDragOver, viewerDrop } from '@/lib/drop';

const environment = '10000000-0000-0000-0000-000000000001';
const data = (values: Record<string, string>) => ({ types: Object.keys(values), getData: (type: string) => values[type] ?? '' }) as unknown as DataTransfer;
const conflict = () => new ApiError({ type: 'orch:error:conflict.stale_revision', title: 'Conflict', detail: 'Exists', status: 409, request_id: 'test', code: 'conflict.stale_revision' }, null);
const stat: FileStat = { type: 'file', path: 'assets/a.txt', name: 'a.txt', size: 1, mtime: '2026-10-25T00:00:00Z', mode: 0o644, link_target: null, cached: false, hash: null, media_type: 'text/plain', is_text: true };

describe('paste and drop destinations', () => {
  it('counts protected drag items without trying to read their names', () => {
    const transfer = { types: ['Files'], files: [], items: [{ kind: 'file' }, { kind: 'file' }, { kind: 'file' }] } as unknown as DataTransfer;
    expect(draggedFileCount(transfer)).toBe(3);
    expect(draggedFileCount({ types: ['Files'], files: [], items: [] } as unknown as DataTransfer)).toBeNull();
  });
  it('viewers prevent file navigation and keep ordinary text editing untouched', () => {
    const preventDefault = vi.fn(), stopPropagation = vi.fn();
    const transfer = { types: ['Files'], dropEffect: 'copy' } as unknown as DataTransfer;
    const event = { dataTransfer: transfer, preventDefault, stopPropagation };
    viewerDragOver(event); viewerDrop(event);
    expect(transfer.dropEffect).toBe('none'); expect(preventDefault).toHaveBeenCalledTimes(2); expect(stopPropagation).toHaveBeenCalledOnce();
    preventDefault.mockClear(); viewerDrop({ ...event, dataTransfer: data({ 'text/plain': 'typing' }) }); expect(preventDefault).not.toHaveBeenCalled();
  });
  it('an unclaimed page drop cannot navigate away and an explicit native file chooser still works', () => {
    const preventDefault = vi.fn();
    const event = { dataTransfer: { types: ['Files'] }, defaultPrevented: false, target: document.body, preventDefault };
    unclaimedFileDrop(event as unknown as DragEvent); expect(preventDefault).toHaveBeenCalledOnce();
    preventDefault.mockClear(); const chooser = document.createElement('input'); chooser.type = 'file';
    unclaimedFileDrop({ ...event, target: chooser } as unknown as DragEvent); expect(preventDefault).not.toHaveBeenCalled();
    unclaimedFileDrop({ ...event, defaultPrevented: true } as unknown as DragEvent); expect(preventDefault).not.toHaveBeenCalled();
  });
  it('keeps a file beside a folder and stops folder enumeration before starting any upload', async () => {
    const cancel = new AbortController();
    const second = vi.fn((done: (file: File) => void) => done(new File(['second'], 'second.txt')));
    const children = [
      { isFile: true, isDirectory: false, name: 'first.txt', file: (done: (file: File) => void) => done(new File(['one'], 'first.txt')) },
      { isFile: true, isDirectory: false, name: 'second.txt', file: second },
    ];
    const folder = { isDirectory: true, isFile: false, name: 'docs', createReader: () => { let read = false; return { readEntries: (done: (entries: unknown[]) => void) => { done(read ? [] : children); read = true; } }; } };
    const transfer = { types: ['Files'], files: [], items: [{ webkitGetAsEntry: () => folder }, { webkitGetAsEntry: () => null, getAsFile: () => new File(['outside'], 'outside.txt') }] } as unknown as DataTransfer;
    await expect(droppedFiles(transfer, cancel.signal, () => cancel.abort())).rejects.toBeDefined();
    expect(second).not.toHaveBeenCalled();
    const files = await droppedFiles(transfer);
    expect(files.map((file) => file.webkitRelativePath || file.name)).toEqual(['docs/first.txt', 'docs/second.txt', 'outside.txt']);
  });
  it('retains a workspace reference rather than treating an internal drag as an upload', () => {
    const value = data({ [ORCH_PATHS]: JSON.stringify({ environmentId: environment, paths: ['src/a file.txt'] }) });
    expect(carriesFiles(value)).toBe(false);
    expect(droppedReference(value)).toBe(`[a file.txt](<id:${environment}/src/a file.txt>)`);
    expect(droppedReference(data({ [ORCH_REFERENCE]: '[Notes](id:document)' }))).toBe('[Notes](id:document)');
    expect(droppedReference(data({ [ORCH_PATHS]: '{bad json' }))).toBeNull();
  });

  it('puts a reference at the selection and retains text on both sides', () => {
    const input = document.createElement('textarea'); input.value = 'one two three'; input.setSelectionRange(4, 7);
    expect(insertAtSelection(input.value, '`file.txt`', input)).toEqual({ text: 'one `file.txt` three', caret: 14 });
  });

  it('keeps folder structure while refusing paths outside the chosen folder', () => {
    const file = new File(['data'], 'a.txt'); Object.defineProperty(file, 'webkitRelativePath', { value: 'docs/a.txt' });
    expect(uploadPath('assets', file)).toBe('assets/docs/a.txt');
    for (const name of ['../a.txt', '/a.txt', 'docs/../../a.txt', 'docs\\a.txt']) expect(() => uploadPath('assets', new File(['data'], name))).toThrow();
    expect(numberedPath('assets/.env', 2)).toBe('assets/.env (2)');
    expect(numberedPath('assets/a.tar.gz', 2)).toBe('assets/a.tar (2).gz');
  });

  it('keeps both when a name is taken, including a name taken during the upload', async () => {
    const write = vi.fn((path: string) => path.endsWith('a.txt') || path.endsWith('a (2).txt') ? Promise.reject(conflict()) : Promise.resolve(stat));
    const choose = vi.fn(() => Promise.resolve('keep' as const));
    await writeUpload('assets/a.txt', new AbortController().signal, write, choose);
    expect(write.mock.calls.map(([path]) => path)).toEqual(['assets/a.txt', 'assets/a (2).txt', 'assets/a (3).txt']);
    expect(write).toHaveBeenLastCalledWith('assets/a (3).txt', 'none');
  });

  it('replaces only after the choice and cancellation prevents the final write', async () => {
    const cancel = new AbortController();
    const write = vi.fn((_path: string, expected: string | null) => expected === 'none' ? Promise.reject(conflict()) : Promise.resolve(stat));
    await writeUpload('assets/a.txt', cancel.signal, write, () => Promise.resolve('replace'));
    expect(write).toHaveBeenLastCalledWith('assets/a.txt', null);
    write.mockClear();
    await expect(writeUpload('assets/a.txt', cancel.signal, write, () => { cancel.abort(); return Promise.resolve('replace'); })).rejects.toBeDefined();
    expect(write).toHaveBeenCalledTimes(1);
  });
});
