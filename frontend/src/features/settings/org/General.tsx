/** Org identity and, for one that lies in another, its address and Move to…; its owners; deleting it, with where its people go and what becomes of what belongs to it; and what was recently deleted.
 *
 * The same page serves a project and a topic, whose general settings are their name and description,
 * where they lie, archiving and deleting. Each act that changes what happens to work says first, on
 * the page and again in the dialog it opens, what it does and until when it can be taken back. What a
 * reader may not do stays on the page, switched off, saying what it takes and whom to ask.
 */
import { useQueryClient } from '@tanstack/react-query';
import { Archive, ArchiveRestore, History, MoveRight, Trash2 } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { applyRow } from '@/api/cache';
import { readNode, updateNode } from '@/api/nodes';
import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { placeOfNode } from '@/features/navigation/places';
import { AutoSaveFeedback, useAutoSave } from '@/features/settings/AutoSave';
import { takes } from '@/features/settings/refusals';
import type { Org, Project, Topic } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { boxClasses, Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { Input } from '@/ui/Input';
import { PermissionGate } from '@/ui/PermissionGate';
import { Textarea } from '@/ui/Textarea';
import { UserChip } from '@/ui/UserChip';

export type SettingsNode = Org | Project | Topic;
/** The node route's NAME_MAX_CHARS; count Unicode scalars as Rust does. */
const NODE_NAME_MAX_CHARS = 120;

/** The last name of a place's path before itself: where it lies, for the line under its heading. */
function whereItLies(node: SettingsNode, separator: string): string {
  return node.path
    .slice(0, -1)
    .map((entry) => entry.name ?? '')
    .join(separator);
}

function Identity({ node }: { node: SettingsNode }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const mayChange = holds(node.access, `${node.kind}.manage`);
  const [left, setLeft] = useState(false);
  const save = useAutoSave({
    current: { value: { name: node.name, description: node.description }, revision: node.revision }, enabled: mayChange,
    reload: async () => { const accepted = await readNode(node.id); applyRow(queryClient, { kind: 'node', id: accepted.id, value: { ...accepted } }); },
    valid: (value) => value.name.trim() !== '' && Array.from(value.name.trim()).length <= NODE_NAME_MAX_CHARS,
    write: async (value, before) => {
      const accepted = await updateNode(node.id, { expected_revision: typeof before.revision === 'number' ? before.revision : node.revision,
        ...(value.name.trim() === before.value.name ? {} : { name: value.name.trim() }),
        ...(value.description === before.value.description ? {} : { description: value.description }) });
      applyRow(queryClient, { kind: 'node', id: accepted.id, value: { ...accepted } });
      return { value: { name: accepted.name, description: accepted.description }, revision: accepted.revision };
    },
  });
  const { name, description } = save.value;
  const long = Array.from(name.trim()).length > NODE_NAME_MAX_CHARS;
  const wrong = name.trim() === '' ? t('settings.general.nameMissing') : long ? t('settings.auto.nameLong', { most: NODE_NAME_MAX_CHARS }) : undefined;
  return (
    <form
      noValidate
      className="flex max-w-(--container-form) flex-col gap-4"
      onSubmit={(event) => {
        event.preventDefault();
        setLeft(true);
        save.commit();
      }}
      onBlur={(event) => { if (!event.currentTarget.contains(event.relatedTarget)) save.commit(); }}
      onCompositionStart={() => save.composing(true)} onCompositionEnd={() => save.composing(false)}
    >
      <Input
        label={t('settings.general.name')}
        value={name}
        readOnly={!mayChange}
        disabled={save.reloading}
        error={left || long ? wrong : undefined}
        onBlur={() => setLeft(true)}
        onChange={(event) => save.edit({ ...save.value, name: event.target.value })}
        autoComplete="off"
      />
      {node.kind === 'org' && (
        <Input
          label={t('settings.general.shortName')}
          hint={node.org_kind === 'personal' ? t('settings.general.shortNamePersonal') : t('settings.general.shortNameHint')}
          value={node.slug}
          readOnly
          mono
        />
      )}
      <Textarea label={t('settings.general.description')} optional value={description} readOnly={!mayChange} disabled={save.reloading} rows={3} onChange={(event) => save.edit({ ...save.value, description: event.target.value })} />
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2 text-sm text-text-muted">
        <span>{t('settings.general.ownedBy')}</span>
        <UserChip user={node.owner} size={24} showUsername thing={{ kind: node.kind, id: node.id }} />
        {node.owners > 1 && <span>{t('settings.general.andOthers', { count: node.owners - 1 })}</span>}
      </div>
      {!mayChange && <p className="text-sm text-text-muted">{takes(t, `${node.kind}.manage`, node.name, node.owner)}</p>}
      <AutoSaveFeedback save={save} name={t('settings.general.identity')} />
    </form>
  );
}

/** One act that changes what becomes of a place: what it does, said before the button that does it. */
function Act({
  title,
  about,
  button,
  note,
}: {
  title: string;
  about: string;
  button: ReactNode;
  note?: ReactNode;
}) {
  return (
    <div className="flex flex-wrap items-center gap-x-6 gap-y-3 px-4 py-3">
      <div className="min-w-0 flex-1 basis-[min(100%,16rem)]">
        <p className="text-base font-medium text-text">{title}</p>
        <p className="text-sm text-text-muted">{about}</p>
        {note}
      </div>
      {button}
    </div>
  );
}

export function General({ node }: { node: SettingsNode }) {
  const { t } = useTranslation();
  const place = placeOfNode(node);
  const manage = `${node.kind}.manage`;
  const may = holds(node.access, manage);
  const reason = takes(t, manage, node.name, node.owner);
  const personal = node.kind === 'org' && node.org_kind === 'personal';
  const inside = node.kind === 'org' ? node.parent_id !== null : true;
  const lies = whereItLies(node, t('settings.place.separator'));
  // An org is not archived; a project and a topic are, and brought back from the same place.
  const archivable = node.kind !== 'org';
  return (
    <div className="flex flex-col gap-6">
      <Section id="general-identity" title={t('settings.general.identity')} lead={t('settings.general.identityLead', { kind: t(`settings.general.kinds.${node.kind}`) })}>
        <div className={`${boxClasses} p-4`}>
          <Identity key={node.id} node={node} />
        </div>
      </Section>

      {inside && (
        <Section id="general-where" title={t('settings.general.where')} lead={lies === '' ? undefined : t('settings.general.lies', { path: lies })}>
          <div className={`${boxClasses} divide-y divide-border`}>
            {node.kind === 'org' && (
              <div className="flex flex-wrap items-center gap-x-6 gap-y-1 px-4 py-3 text-base">
                <span className="text-text-muted">{t('settings.general.address')}</span>
                <span className="font-mono text-text">{node.address}</span>
              </div>
            )}
            <Act
              title={t('settings.general.moveTitle')}
              about={t('settings.general.moveAbout', { kind: t(`settings.general.kinds.${node.kind}`) })}
              button={
                <PermissionGate allowed={may} mode="block" reason={reason}>
                  <Button icon={<MoveRight aria-hidden />} onClick={() => openNodeDialog({ type: 'move', node: place })}>
                    {t('settings.general.move')}
                  </Button>
                </PermissionGate>
              }
            />
          </div>
        </Section>
      )}

      {!personal && (
        <Section id="general-danger" title={archivable ? t('settings.general.danger') : t('settings.general.dangerNoArchive')}>
          <div className={`${boxClasses} divide-y divide-border`}>
            {archivable && (
              <Act
                title={node.archived ? t('settings.general.archivedTitle') : t('settings.general.archiveTitle')}
                about={node.archived ? t('settings.general.archivedAbout') : t(`places.archive.body.${node.kind}`)}
                button={
                  <PermissionGate allowed={holds(node.access, `${node.kind}.archive`) || may} mode="block" reason={takes(t, `${node.kind}.archive`, node.name, node.owner)}>
                    <Button
                      icon={node.archived ? <ArchiveRestore aria-hidden /> : <Archive aria-hidden />}
                      onClick={() => openNodeDialog({ type: 'archive', node: place })}
                    >
                      {node.archived ? t('settings.general.bringBack') : t('settings.general.archive')}
                    </Button>
                  </PermissionGate>
                }
              />
            )}
            <Act
              title={t('settings.general.deleteTitle', { name: node.name })}
              about={t('settings.general.deleteAbout', { kind: t(`settings.general.kinds.${node.kind}`) })}
              note={
                node.kind === 'org' && node.parent_id !== null ? (
                  <p className="mt-1 text-sm text-text-muted">{t('settings.general.deleteDepartment', { count: node.member_count })}</p>
                ) : undefined
              }
              button={
                <PermissionGate allowed={may} mode="block" reason={reason}>
                  <Button variant="danger" icon={<Trash2 aria-hidden />} onClick={() => openNodeDialog({ type: 'delete', node: place })}>
                    {t('settings.general.delete')}
                  </Button>
                </PermissionGate>
              }
            />
            <Act
              title={t('settings.general.deletedTitle')}
              about={t('settings.general.deletedAbout')}
              button={
                <PermissionGate allowed={may} mode="block" reason={reason}>
                  <Button icon={<History aria-hidden />} onClick={() => openNodeDialog({ type: 'deleted', node: place })}>
                    {t('settings.general.deleted')}
                  </Button>
                </PermissionGate>
              }
            />
          </div>
        </Section>
      )}
    </div>
  );
}
