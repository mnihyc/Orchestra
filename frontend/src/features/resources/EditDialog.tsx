/** The frame of the dialog that edits a resource: the name and what its kind lets be changed after it was made, and under them what cannot be changed, read only in the same form with the reason.
 *
 * Every kind of resource is edited the same way, so that nobody has to learn where a screen changes
 * its own things. The kind brings its fields; this brings the form around them: what was changed is saved
 * together, under the revision that was read, and a change that somebody else made first is said, not
 * overwritten. What is fixed once the thing is made, and what is changed in place on another tab of its
 * page, stand in the same form as quiet fields with the reason or the way to the tab, so that the form
 * says everything about the thing and nothing in it is mistaken for a field that can be changed.
 */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { linkButtonClasses } from '@/ui/link';
import type { AdminActSpec } from '@/ui/AdminAct';

export interface EditDialogProps {
  admin?: AdminActSpec;
  title: string;
  /** Creation uses its own verb while the same frame keeps editing's Save. */
  submitLabel?: string;
  description?: string;
  /** Whether what is typed differs from what is kept: nothing is saved that was not changed. */
  changed: boolean;
  /** Whether what is typed can be saved at all (a name is not empty, a text is valid). */
  valid?: boolean;
  /** What the save is under way for. */
  pending: boolean;
  /** What the save was refused with. */
  error: unknown;
  onSave: () => void;
  onClose: () => void;
  /** The fields that can be changed. */
  children: ReactNode;
  /** What cannot be changed here: `Fixed` and `ChangedOn` rows. */
  rest?: ReactNode;
}

export function EditDialog({ admin, title, submitLabel, description, changed, valid = true, pending, error, onSave, onClose, children, rest }: EditDialogProps) {
  const { t } = useTranslation();
  const stale = error instanceof ApiError && error.is('conflict.stale_revision');
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !pending && onClose()}
      title={title}
      {...(description === undefined ? {} : { description })}
      dismissible={!pending}
      footer={
        <>
          <Button disabled={pending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button admin={admin} variant="primary" type="submit" form="edit-resource" pending={pending} blocked={!changed || !valid}>
            {submitLabel ?? t('resources.edit.save')}
          </Button>
        </>
      }
    >
      <form
        id="edit-resource"
        className="flex flex-col gap-4 py-2"
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          if (changed && valid && !pending) onSave();
        }}
      >
        {children}
        {rest !== undefined && (
          <fieldset className="flex flex-col gap-4 border-t border-border pt-4">
            <legend className="sr-only">{t('resources.edit.rest')}</legend>
            {rest}
          </fieldset>
        )}
        {stale && (
          <InlineAlert tone="warning" title={t('resources.edit.changed')}>
            {t('resources.edit.changedLead')}
          </InlineAlert>
        )}
        {error !== null && error !== undefined && !stale && <ErrorAlert error={error} />}
      </form>
    </Dialog>
  );
}

/** What cannot be changed after the thing was made, shown as it is, with the reason beside it. */
export function Fixed({ label, value, why }: { label: string; value: string; why: string }) {
  return <Input label={label} value={value} readOnly hint={why} />;
}

/** What is changed in place on another tab of the thing's page: its value, and the way to that tab. */
export function ChangedOn({ label, value, tab, onOpen }: { label: string; value: string; tab: string; onOpen: () => void }) {
  const { t } = useTranslation();
  return (
    <Input
      label={label}
      value={value}
      readOnly
      hint={t('resources.edit.changedOn', { tab })}
      aside={
        <button type="button" className={linkButtonClasses} onClick={onOpen}>
          {t('resources.edit.goTo', { tab })}
        </button>
      }
    />
  );
}
