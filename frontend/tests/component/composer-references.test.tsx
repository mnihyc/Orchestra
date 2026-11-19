/** The plus menu and shared reference picker add a path or a permission-checked document copy to the draft without sending it. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import type * as SearchApi from '@/api/search';
import type * as DocumentsApi from '@/api/documents';
import type * as WorkspaceApi from '@/api/workspace';
import { AttachButton } from '@/features/composer/Attachments';
import { ReferencePicker } from '@/features/composer/Mentions';
import type { DirEntry, NameHit, Thread } from '@/generated/api-types';
import { show } from './support';

const calls = vi.hoisted(() => ({ directory: vi.fn<typeof WorkspaceApi.listDirectory>(), search: vi.fn<typeof WorkspaceApi.searchNames>(),
  export: vi.fn<typeof DocumentsApi.exportDocument>(), file: vi.fn<(path: string) => void>(), document: vi.fn<(file: File) => void>(), close: vi.fn(), upload: vi.fn(), pickFile: vi.fn(), pickDocument: vi.fn() }));
vi.mock('@/api/workspace', async (importOriginal) => ({ ...await importOriginal<typeof WorkspaceApi>(), listDirectory: calls.directory, searchNames: calls.search }));
vi.mock('@/api/documents', async (importOriginal) => ({ ...await importOriginal<typeof DocumentsApi>(), exportDocument: calls.export }));
vi.mock('@/api/search', async (importOriginal) => ({ ...await importOriginal<typeof SearchApi>(), namesQuery: (search: SearchApi.NameSearch) => ({ queryKey: ['reference-documents', search.q], queryFn: () => Promise.resolve([document]), staleTime: Infinity }) }));

const document: NameHit = { id: 'doc-1', kind: 'document', name: 'Login design', address: null, username: null, archived: false, avatar: null, color: null, custom_color: null,
  owner: null, recent: true, path: [{ id: 'org-1', kind: 'org', name: 'Acme', redacted: false }], tag: null, thread_attention: null };
const file: DirEntry = { type: 'file', name: 'login.ts', path: 'src/login.ts', size: 12, mtime: '2026-10-15T00:00:00Z', mode: 0o644, link_target: null, cached: false };
const thread = { id: 'thread-1', environment: { id: 'env-1' }, directory: 'src', environment_permissions: ['files.view'] } as Thread;

beforeEach(() => {
  for (const call of [calls.directory, calls.search, calls.export, calls.file, calls.document, calls.close, calls.upload, calls.pickFile, calls.pickDocument]) call.mockReset();
  calls.directory.mockResolvedValue({ path: 'src', entries: [file, { ...file, type: 'dir', name: 'tests', path: 'src/tests' }], as_of: file.mtime, mount: null, source: 'live', truncated: false });
  calls.search.mockResolvedValue([{ ...file, path: 'src/auth/login.ts' }]);
  calls.export.mockResolvedValue({ name: 'login-design.md', blob: new Blob(['# Login design\nUse redirects.'], { type: 'text/markdown' }), losses: [] });
});

it('offers attach files and both references from +, retaining the file-access reason', async () => {
  show(<AttachButton onPress={calls.upload} onFile={calls.pickFile} onDocument={calls.pickDocument} fileReason="This environment does not allow reading files." />);
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Add to message' }));
  expect(screen.getByRole('menuitem', { name: /^Mention a workspace file/ })).toHaveAttribute('aria-disabled', 'true');
  await user.click(screen.getByRole('menuitem', { name: 'Attach files…' }));
  expect(calls.upload).toHaveBeenCalledTimes(1);
  await user.click(screen.getByRole('button', { name: 'Add to message' }));
  await user.click(screen.getByRole('menuitem', { name: 'Mention a document…' }));
  expect(calls.pickDocument).toHaveBeenCalledTimes(1);
  expect(calls.pickFile).not.toHaveBeenCalled();
});

it('reads the workspace under this thread and adds only the selected existing file path', async () => {
  show(<ReferencePicker thread={thread} initialKind="file" onFile={calls.file} onDocument={calls.document} onClose={calls.close} />);
  const choice = await screen.findByRole('button', { name: 'src/login.ts' });
  await waitFor(() => expect(choice).toBeEnabled());
  const requested = calls.directory.mock.calls[0];
  expect(requested?.[0]).toEqual({ environmentId: 'env-1', threadId: thread.id });
  expect(requested?.[1]).toBe('src');
  expect(requested?.[2]?.signal).toBeInstanceOf(AbortSignal);
  expect(screen.queryByRole('button', { name: 'src/tests' })).not.toBeInTheDocument();
  await userEvent.click(choice);
  expect(calls.file).toHaveBeenCalledExactlyOnceWith('src/login.ts');
  expect(calls.close).toHaveBeenCalledTimes(1);
  expect(calls.export).not.toHaveBeenCalled();
  expect(calls.document).not.toHaveBeenCalled();
});

it('searches a typed file name and uses the returned path instead of inventing a path', async () => {
  show(<ReferencePicker thread={thread} initialKind="file" initialQuery="login" onFile={calls.file} onDocument={calls.document} onClose={calls.close} />);
  const choice = await screen.findByRole('button', { name: 'src/auth/login.ts' });
  await waitFor(() => expect(choice).toBeEnabled());
  expect(calls.search).toHaveBeenCalledWith({ environmentId: 'env-1', threadId: thread.id }, 'login', expect.any(AbortSignal));
  await userEvent.click(choice);
  expect(calls.file).toHaveBeenCalledExactlyOnceWith('src/auth/login.ts');
});

it('does not read workspace names without the environment permission', () => {
  show(<ReferencePicker thread={{ ...thread, environment_permissions: [] }} initialKind="file" onFile={calls.file} onDocument={calls.document} onClose={calls.close} />);
  expect(screen.getByText('Reading these files needs access to the environment’s files.')).toBeVisible();
  expect(calls.directory).not.toHaveBeenCalled();
  expect(calls.search).not.toHaveBeenCalled();
});

it('copies a chosen readable document into a draft attachment and never sends the message', async () => {
  show(<ReferencePicker thread={thread} initialKind="document" onFile={calls.file} onDocument={calls.document} onClose={calls.close} />);
  const choice = await screen.findByRole('button', { name: /Login design.*Acme/ });
  await waitFor(() => expect(choice).toBeEnabled());
  await userEvent.click(choice);
  await waitFor(() => expect(calls.document).toHaveBeenCalledTimes(1));
  expect(calls.export).toHaveBeenCalledExactlyOnceWith(document.id, { format: 'markdown', attachments: 'omit', comments: 'omit' });
  const attached = calls.document.mock.calls[0]?.[0];
  expect(attached).toBeInstanceOf(File);
  expect(attached?.name).toBe('login-design.md');
  expect(attached?.type).toBe('text/markdown');
  expect(attached?.size).toBeGreaterThan(0);
  expect(calls.file).not.toHaveBeenCalled();
  expect(calls.close).toHaveBeenCalledTimes(1);
});

it('keeps a refused document read visible without attaching a substitute', async () => {
  calls.export.mockRejectedValue(new ApiError({ type: 'orch:error:authz.denied', status: 403, code: 'authz.denied', title: 'Access denied', detail: 'This document is no longer shared with you.', request_id: 'r1' }, null));
  show(<ReferencePicker thread={thread} initialKind="document" onFile={calls.file} onDocument={calls.document} onClose={calls.close} />);
  const choice = await screen.findByRole('button', { name: /Login design/ });
  await waitFor(() => expect(choice).toBeEnabled());
  await userEvent.click(choice);
  expect(await screen.findByText('You do not have access to this. Ask somebody who manages it.')).toBeVisible();
  expect(calls.document).not.toHaveBeenCalled();
  expect(calls.close).not.toHaveBeenCalled();
});
