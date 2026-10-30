/** Who gains and who loses what, before a row is cut loose, an entry removed, a default changed or a lock set.
 *
 * A change that takes something away, or that reaches other pages, is asked about once, in a dialog
 * that says what it does and, from the server, whom it reaches: how many pages follow the rows
 * changed, and who gains or loses which permissions on them. A failed preview shows its cause and
 * can be retried; an empty answer is never made up. The change is
 * made only when the person confirms, and the dialog stays, with its refusal, until the answer.
 */
import { useQuery } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import type { PageChange, PagePreview, PermissionPage, PreviewPerson } from '@/generated/api-types';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { UserChip } from '@/ui/UserChip';

import { type Changes, type PageSource, sourceKey, sourcePreview, sourceQuery } from './changes';
import { usePermissionName } from './names';

/** An act the person has asked for and is to confirm. */
export interface Asked {
  /** Names the thing and the act: "Make Fix login private?" */
  title: string;
  /** What it does, in words, before the server's account of whom it reaches. */
  body: ReactNode;
  /** The verb and the name once more: "Make Fix login private". */
  confirmLabel: string;
  danger?: boolean;
  /** What is being done, for the line that says so while it is. */
  label: string;
  make: (page: PermissionPage) => PageChange;
  /** Whether the server is asked whom it reaches; a change that takes nothing away does not ask. */
  preview?: boolean;
  /** Said after the confirmation is made, in place of nothing: shown in a toast by the caller. */
  after?: () => void;
}

/** How many people gain or lose, and the names behind the count, which are opened by whoever wants them. */
function Reached({ people, tone }: { people: readonly PreviewPerson[]; tone: 'gain' | 'lose' }) {
  const { t } = useTranslation();
  const permissionName = usePermissionName();
  if (people.length === 0) return null;
  return (
    <details className="group flex flex-col gap-1">
      <summary className="cursor-default rounded-md text-sm font-medium text-text select-none hover:text-accent-text">
        {t(tone === 'gain' ? 'access.preview.gain' : 'access.preview.lose', { count: people.length })}
      </summary>
      <ul className="mt-1 flex flex-col gap-1">
        {people.map((person) => (
          <li key={person.user.id} className="flex flex-wrap items-center gap-x-2 text-sm text-text-muted">
            <UserChip user={person.user} size={24} />
            <span className="min-w-0 wrap-anywhere">{person.permissions.map(permissionName).join(', ')}</span>
          </li>
        ))}
      </ul>
    </details>
  );
}

/** What the server says the change reaches: how many pages, in how many orgs where it is the system's own defaults, and the people who gain or lose with the names behind the counts. */
export function Account({ preview }: { preview: PagePreview }) {
  const { t } = useTranslation();
  const none = preview.gaining.length === 0 && preview.losing.length === 0 && preview.more === 0;
  return (
    <div className="flex flex-col gap-2" aria-live="polite">
      {preview.pages > 1 && (
        <p className="text-sm text-text-muted">
          {t('access.preview.pages', { count: preview.pages })}
          {preview.orgs !== null && ` ${t('access.preview.orgs', { count: preview.orgs })}`}
        </p>
      )}
      {none ? <p className="text-sm text-text">{t('access.preview.nobody')}</p> : null}
      <Reached people={preview.losing} tone="lose" />
      <Reached people={preview.gaining} tone="gain" />
      {preview.more > 0 && <p className="text-sm text-text-muted">{t('access.preview.more', { count: preview.more })}</p>}
    </div>
  );
}

export function ChangePreview({ source, asked, changes, onClose }: { source: PageSource; asked: Asked | null; changes: Changes; onClose: () => void }) {
  const { t } = useTranslation();
  const wanted = asked !== null && asked.preview !== false;
  const page = useQuery({ ...sourceQuery(source), enabled: asked !== null });
  const preview = useQuery({
    queryKey: ['preview', ...sourceKey(source), asked?.label ?? '', page.data?.revision ?? 0],
    queryFn: () => {
      const current = page.data;
      if (asked === null || current === undefined) throw new Error('nothing to preview');
      return sourcePreview(source, asked.make(current));
    },
    enabled: wanted && page.data !== undefined,
    retry: false,
    staleTime: 0,
    gcTime: 0,
  });

  return (
    <ConfirmDialog
      open={asked !== null}
      onOpenChange={(next) => !next && onClose()}
      title={asked?.title ?? ''}
      confirmLabel={asked?.confirmLabel ?? ''}
      danger={asked?.danger ?? false}
      pending={changes.busy}
      disabled={wanted && (page.isPending || page.isError || preview.isPending || preview.isError || preview.isFetching)}
      error={changes.error}
      onConfirm={() => {
        if (asked === null) return;
        void changes.run({ label: asked.label, make: asked.make }).then((made) => {
          if (!made) return;
          asked.after?.();
          onClose();
        });
      }}
    >
      <div className="flex flex-col gap-3">
        <div>{asked?.body}</div>
        {wanted &&
          (page.isError ? (
            <ErrorState compact error={page.error} action={<Button size="sm" action="retry" pending={page.isRefetching} onClick={() => void page.refetch()}>{t('common.tryAgain')}</Button>} />
          ) : preview.isPending ? (
            <div>
              <p className="sr-only">{t('access.preview.working')}</p>
              <SkeletonRows rows={2} />
            </div>
          ) : preview.isError ? (
            <ErrorState compact error={preview.error} action={<Button size="sm" action="retry" pending={preview.isRefetching} onClick={() => void preview.refetch()}>{t('common.tryAgain')}</Button>} />
          ) : (
            <Account preview={preview.data} />
          ))}
      </div>
    </ConfirmDialog>
  );
}
