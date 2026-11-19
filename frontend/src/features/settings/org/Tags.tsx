/** Topic tags.
 *
 * The tags that topics here can carry: a name and one of the twelve colours of the identity palette.
 * A place lists its own, which it changes, and those of the orgs above it, which it can only use. A
 * tag taken away is removed from every topic that carried it, and the dialog says how many places
 * that is not known to be before it asks: the server removes it from all of them.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Pencil, Plus, Tags as TagsIcon, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { createTag, deleteTag, updateTag } from '@/api/directory';
import { tagsQuery } from '@/api/nodes';
import { takes } from '@/features/settings/refusals';
import type { SettingsNode } from '@/features/settings/org/General';
import type { Tag } from '@/generated/api-types';
import { IDENTITY_COLORS } from '@/lib/grapheme';
import { useActKey } from '@/lib/forms';
import { holds } from '@/lib/permissions';
import { boxClasses, Section } from '@/shell/Page';
import { identityGround } from '@/ui/Avatar';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { cx } from '@/ui/cx';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { PermissionGate } from '@/ui/PermissionGate';
import { SkeletonRows } from '@/ui/Skeleton';
import { TagChip } from '@/ui/TagChip';
import { toast } from '@/ui/Toast';

/** The colour a new tag begins with: the first of the palette, until its maker chooses. */
const FIRST_COLOR = 0;

/** The names of the colours, in the order of the palette; a colour is never the only thing that tells tags apart, but it needs a name to be chosen by. */
const COLOR_NAMES = ['red', 'orange', 'amber', 'lime', 'green', 'teal', 'cyan', 'blue', 'indigo', 'purple', 'pink', 'grey'] as const;

function ColorChoice({ value, onChange }: { value: number; onChange: (color: number) => void }) {
  const { t } = useTranslation();
  return (
    <fieldset className="flex flex-col gap-2">
      <legend className="text-sm font-medium text-text">{t('tags.color')}</legend>
      <div className="flex flex-wrap gap-2" role="radiogroup" aria-label={t('tags.color')}>
        {Array.from({ length: IDENTITY_COLORS }, (_, index) => {
          const name = t(`tags.colors.${COLOR_NAMES[index] ?? 'grey'}`);
          return (
            <button
              key={index}
              type="button"
              role="radio"
              aria-checked={value === index}
              aria-label={name}
              title={name}
              onClick={() => onChange(index)}
              className={cx(
                'size-6 rounded-full border-2 transition-shadow',
                identityGround(index),
                value === index ? 'border-text shadow-sm ring-2 ring-focus ring-offset-1 ring-offset-surface' : 'border-transparent hover:border-text-subtle',
              )}
            />
          );
        })}
      </div>
    </fieldset>
  );
}

/** Makes a tag, or changes one's name and colour. */
function TagDialog({ node, tag, onClose }: { node: SettingsNode; tag: Tag | null; onClose: () => void }) {
  const { t } = useTranslation();
  const actKey = useActKey();
  const [name, setName] = useState(tag?.name ?? '');
  const [color, setColor] = useState(tag?.color ?? FIRST_COLOR);
  const [sent, setSent] = useState(false);
  const wrong = name.trim() === '' ? t('tags.nameMissing') : undefined;
  const save = useMutation({
    mutationFn: () => {
      const body = { name: name.trim(), color };
      return tag === null ? createTag(node.id, body, actKey(body)) : updateTag(tag.id, body);
    },
    onSuccess: () => {
      toast({ tone: 'success', title: tag === null ? t('tags.made', { name: name.trim() }) : t('tags.changed', { name: name.trim() }) });
      onClose();
    },
  });
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !save.isPending && onClose()}
      title={tag === null ? t('tags.newTitle') : t('tags.editTitle', { name: tag.name })}
      description={t('tags.dialogLead', { place: node.name })}
      dismissible={!save.isPending}
      footer={
        <>
          <Button disabled={save.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button
            variant="primary"
            pending={save.isPending}
            onClick={() => {
              setSent(true);
              if (wrong === undefined && !save.isPending) save.mutate();
            }}
          >
            {tag === null ? t('tags.make') : t('settings.save')}
          </Button>
        </>
      }
    >
      <form
        noValidate
        className="flex flex-col gap-4 py-2"
        onSubmit={(event) => {
          event.preventDefault();
          setSent(true);
          if (wrong === undefined && !save.isPending) save.mutate();
        }}
      >
        <Input label={t('tags.name')} value={name} error={sent ? wrong : undefined} onChange={(event) => setName(event.target.value)} autoComplete="off" />
        <ColorChoice value={color} onChange={setColor} />
        <div className="flex items-center gap-2 text-sm text-text-muted">
          {t('tags.preview')} <TagChip name={name.trim() === '' ? t('tags.previewName') : name.trim()} color={color} />
        </div>
        {save.error !== null && <ErrorAlert error={save.error} />}
      </form>
    </Dialog>
  );
}

export function Tags({ node }: { node: SettingsNode }) {
  const { t } = useTranslation();
  const list = useQuery(tagsQuery(node.id));
  const [editing, setEditing] = useState<Tag | 'new' | null>(null);
  const [removing, setRemoving] = useState<Tag | null>(null);
  const may = holds(node.access, `${node.kind}.manage`);
  const reason = takes(t, `${node.kind}.manage`, node.name, node.owner);
  const remove = useMutation({
    mutationFn: (tag: Tag) => deleteTag(tag.id),
    onSuccess: (_none, tag) => {
      toast({ tone: 'success', title: t('tags.removed', { name: tag.name }) });
      setRemoving(null);
    },
  });
  const own = (list.data ?? []).filter((tag) => tag.node.id === node.id);
  const above = (list.data ?? []).filter((tag) => tag.node.id !== node.id);
  return (
    <Section
      id="tags"
      title={t('settings.org.sectionNames.tags')}
      lead={t('tags.lead', { place: node.name })}
      action={
        <PermissionGate allowed={may} mode="block" reason={reason}>
          <Button size="sm" variant="primary" icon={<Plus aria-hidden />} onClick={() => setEditing('new')}>
            {t('tags.new')}
          </Button>
        </PermissionGate>
      }
    >
      {list.isPending ? (
        <SkeletonRows rows={3} />
      ) : list.isError ? (
        <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
      ) : list.data.length === 0 ? (
        <EmptyState compact icon={<TagsIcon aria-hidden />} title={t('tags.none', { place: node.name })} description={may ? t('tags.noneAbout') : undefined} />
      ) : (
        <div className="flex flex-col gap-4">
          {own.length > 0 && (
            <ul className={`${boxClasses} divide-y divide-border`} aria-label={t('tags.ownList', { place: node.name })}>
              {own.map((tag) => (
                <li key={tag.id} className="flex items-center gap-3 px-4 py-2">
                  <TagChip name={tag.name} color={tag.color} />
                  <span className="flex-1" />
                  {may && (
                    <>
                      <IconButton size="sm" label={t('tags.edit', { name: tag.name })} icon={<Pencil aria-hidden />} onClick={() => setEditing(tag)} />
                      <IconButton size="sm" label={t('tags.remove', { name: tag.name })} icon={<Trash2 aria-hidden />} onClick={() => setRemoving(tag)} />
                    </>
                  )}
                </li>
              ))}
            </ul>
          )}
          {above.length > 0 && (
            <div className="flex flex-col gap-2">
              <h3 className="text-sm font-medium text-text-muted">{t('tags.fromAbove')}</h3>
              <ul className={`${boxClasses} divide-y divide-border`} aria-label={t('tags.aboveList')}>
                {above.map((tag) => (
                  <li key={tag.id} className="flex flex-wrap items-center gap-3 px-4 py-2">
                    <TagChip name={tag.name} color={tag.color} />
                    <span className="text-sm text-text-muted">{t('tags.setIn', { place: tag.node.name ?? '' })}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}
      {editing !== null && <TagDialog node={node} tag={editing === 'new' ? null : editing} onClose={() => setEditing(null)} />}
      <ConfirmDialog
        open={removing !== null}
        onOpenChange={(next) => !next && setRemoving(null)}
        title={t('tags.removeTitle', { name: removing?.name ?? '' })}
        confirmLabel={t('tags.removeConfirm', { name: removing?.name ?? '' })}
        danger
        pending={remove.isPending}
        error={remove.error ?? undefined}
        onConfirm={() => removing !== null && remove.mutate(removing)}
      >
        {t('tags.removeBody')}
      </ConfirmDialog>
    </Section>
  );
}
