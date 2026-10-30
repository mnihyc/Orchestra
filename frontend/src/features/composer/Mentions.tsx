/** Workspace files and readable documents added as references to the unsent message.
 * A selected workspace file adds its existing path. A selected document is exported through its
 * normal permission-checked reader and attached to the draft; choosing never sends the message.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { FileText, FolderOpen } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { choiceOption } from '@/i18n/choices';

import { exportDocument } from '@/api/documents';
import { namesQuery } from '@/api/search';
import { listDirectory, searchNames, type Door } from '@/api/workspace';
import { wayOf } from '@/features/finder/destinations';
import type { NameHit, Thread } from '@/generated/api-types';
import { useDebounced } from '@/lib/debounce';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { SegmentedControl } from '@/ui/SegmentedControl';

export type ReferenceKind = 'file' | 'document';

export function ReferencePicker({ thread, initialKind, initialQuery = '', onFile, onDocument, onClose }: {
  thread: Thread;
  initialKind: ReferenceKind;
  initialQuery?: string;
  onFile: (path: string) => void;
  onDocument: (file: File) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [kind, setKind] = useState(initialKind);
  const [text, setText] = useState(initialQuery);
  const query = useDebounced(text.trim());
  const door: Door | null = thread.environment === null ? null : { environmentId: thread.environment.id, threadId: thread.id };
  const mayReadFiles = door !== null && thread.environment_permissions.includes('files.view');
  const files = useQuery({
    queryKey: ['composer-reference-files', thread.id, door?.environmentId, thread.directory, query],
    queryFn: async ({ signal }) => door === null ? [] : query === '' ? (await listDirectory(door, thread.directory, { signal })).entries : searchNames(door, query, signal),
    enabled: kind === 'file' && mayReadFiles,
  });
  const documents = useQuery({ ...namesQuery({ q: query, kinds: ['document'] }), enabled: kind === 'document' });
  const copying = useMutation({
    mutationFn: (document: NameHit) => exportDocument(document.id, { format: 'markdown', attachments: 'omit', comments: 'omit' }),
    onSuccess: ({ blob, name }) => { onDocument(new File([blob], name, { type: 'text/markdown' })); onClose(); },
  });
  const waiting = text.trim() !== query || (kind === 'file' ? files.isFetching : documents.isFetching);
  const fileRows = (files.data ?? []).filter((entry) => entry.type === 'file');
  const documentRows = (documents.data ?? []).filter((entry) => entry.kind === 'document');
  const failure = kind === 'file' ? files.error : documents.error;
  return <Dialog open onOpenChange={(open) => { if (!open && !copying.isPending) onClose(); }} title={t('composer.plus.search')} dismissible={!copying.isPending}>
    <div className="flex flex-col gap-3">
      <SegmentedControl<ReferenceKind> label={t('composer.plus.label')} value={kind} onValueChange={setKind}
        segments={[{ value: 'file', ...choiceOption(t, 'referenceKind.file'), description: t('composer.plus.fileAbout') }, { value: 'document', ...choiceOption(t, 'referenceKind.document'), description: t('composer.plus.documentAbout') }]} />
      <Input label={t('composer.plus.search')} value={text} onChange={(event) => setText(event.target.value)} autoFocus disabled={copying.isPending} />
      {kind === 'file' && !mayReadFiles ? <p className="text-sm text-text-muted">{t(door === null ? 'composer.plus.noEnvironment' : 'composer.plus.noFileAccess')}</p> : <>
        {waiting && <p role="status" className="text-sm text-text-muted">{t('composer.plus.loading')}</p>}
        {failure !== null && <ErrorAlert error={failure} />}
        <ul className="flex max-h-80 flex-col gap-1 overflow-y-auto" aria-label={t('composer.plus.search')} data-scroll-allow="choices">
          {kind === 'file' ? fileRows.map((file) => <li key={file.path}><button type="button" disabled={waiting || copying.isPending} onClick={() => { onFile(file.path); onClose(); }} className="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left hover:bg-bg-muted"><FileText aria-hidden /><span className="min-w-0 break-all text-sm">{file.path}</span></button></li>)
            : documentRows.map((document) => <li key={document.id}><button type="button" disabled={waiting || copying.isPending} onClick={() => copying.mutate(document)} className="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left hover:bg-bg-muted"><FolderOpen aria-hidden /><span className="min-w-0"><span className="block truncate text-sm">{document.name}</span><span className="block truncate text-xs text-text-muted">{wayOf(document, t)}</span></span></button></li>)}
        </ul>
        {!waiting && failure === null && (kind === 'file' ? fileRows.length === 0 : documentRows.length === 0) && <EmptyState compact title={t('composer.plus.nothing')} />}
      </>}
      {copying.isPending && <p role="status">{t('composer.plus.loading')}</p>}
      {copying.error !== null && <ErrorAlert error={copying.error} />}
    </div>
  </Dialog>;
}
