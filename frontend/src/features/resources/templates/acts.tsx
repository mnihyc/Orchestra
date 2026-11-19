/** What can be done with a template, from its row in the list and from its own page: enable or disable, open, edit, rename, copy its link and delete, each only where the reader may.
 *
 * The same acts serve the button and the menu at the end of a row, the menu under the right mouse button, the
 * acts rows that are ticked share, and the menu and the Edit button in the heading of the page. Disabling asks
 * first, naming the template, and only stops new units from being made; a template that units were made from
 * cannot be deleted, and its Delete stays in the menu with that reason.
 */
import { useResourceCleanup } from '@/features/storage/acts';
import { useMutation } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { Ban, CirclePlay, Copy, ArrowRight, Pencil, TextCursorInput, Trash2 } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { deleteTemplate, updateTemplate } from '@/api/resources';
import { type Act, type BulkAct, BulkNames, entriesOf, keysOf, type ListActs, type RowActs, useBusy, useCopyLink, shareAct } from '@/features/resources/acts';
import { EditTemplate } from '@/features/resources/templates/Edit';
import type { TemplateTab } from '@/features/resources/templates/Template';
import type { Template } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { adminSpec } from '@/ui/AdminAct';
import { toast } from '@/ui/Toast';

export interface Asked {
  what: 'edit' | 'rename' | 'disable' | 'delete';
  template: Template;
}

export interface TemplateActs {
  list: ListActs<Template>;
  /** Asks for an act that has a dialog, for a button of the page that is not in the menu. */
  ask: (what: Asked['what'], template: Template) => void;
  /** Enables a template that was disabled, at once. */
  enable: (template: Template) => void;
  page: (template: Template) => { edit: Act | null; primary: Act | null; menu: RowActs['entries'] };
  dialogs: ReactNode;
}

export function useTemplateActs(options: { onTab?: (template: Template, tab: TemplateTab) => void; afterDelete?: () => void } = {}): TemplateActs {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const copyLink = useCopyLink();
  const { busy, track } = useBusy();
  const cleanup = useResourceCleanup();
  const [asked, setAsked] = useState<Asked | null>(null);

  const toTab = (template: Template, tab: TemplateTab) => {
    if (options.onTab !== undefined) options.onTab(template, tab);
    else void navigate({ to: '/resources/templates/$id', params: { id: template.id }, search: tab === 'overview' ? {} : { tab } });
  };
  /** Switches a template on or off, on the revision that was read. */
  const setEnabled = (template: Template, enabled: boolean): Promise<Template> =>
    updateTemplate(template.id, { expected_revision: template.revision, enabled });
  const drop = (template: Template): Promise<void> => deleteTemplate(template.id);

  const switchOff = useMutation({
    mutationFn: (template: Template) => setEnabled(template, false),
    onSuccess: (updated) => {
      toast({ tone: 'success', title: t('templates.danger.disabled', { name: updated.name }) });
      setAsked(null);
    },
  });
  const remove = useMutation({
    mutationFn: drop,
    onSuccess: (_reply, template) => {
      toast({ tone: 'success', title: t('templates.danger.deleted', { name: template.name }) });
      setAsked(null);
      options.afterDelete?.();
    },
  });

  const canDelete = (template: Template) => holds(template.access, 'template.manage') && template.units === 0;

  const actsOf = (template: Template, where: 'row' | 'page'): RowActs & { edit: Act | null } => {
    const manage = holds(template.access, 'template.manage');
    const open: Act | null =
      where === 'row' ? { id: 'open', label: t('resources.act.open'), icon: <ArrowRight aria-hidden />, run: () => void navigate({ to: '/resources/templates/$id', params: { id: template.id } }) } : null;
    const enable: Act | null =
      manage && !template.enabled
        ? {
            id: 'enable',
            label: t('resources.act.enable'),
            icon: <CirclePlay aria-hidden />,
            run: () =>
              track(template.id, async () => {
                const updated = await setEnabled(template, true);
                toast({ tone: 'success', title: t('templates.danger.enabled', { name: updated.name }) });
              }),
          }
        : null;
    const disable: Act | null = manage && template.enabled ? { id: 'disable', label: t('resources.act.disable'), icon: <Ban aria-hidden />, run: () => setAsked({ what: 'disable', template }) } : null;
    const edit: Act | null = manage ? { id: 'edit', label: t('resources.act.edit'), icon: <Pencil aria-hidden />, run: () => setAsked({ what: 'edit', template }) } : null;
    const rename: Act | null = manage ? { id: 'rename', label: t('resources.act.rename'), icon: <TextCursorInput aria-hidden />, shortcut: 'F2', run: () => setAsked({ what: 'rename', template }) } : null;
    const share = shareAct(t, { kind: 'template', id: template.id, name: template.name }, template.access);
    const copy: Act = { id: 'copy-link', label: t('resources.act.copyLink'), icon: <Copy aria-hidden />, run: () => copyLink(`/resources/templates/${template.id}`) };
    const del: Act | null = manage
      ? { id: 'delete', label: t('resources.act.delete'), icon: <Trash2 aria-hidden />, danger: true, disabled: template.units > 0 ? t('templates.danger.deleteUsed', { count: template.units }) : false, run: () => setAsked({ what: 'delete', template }) }
      : null;
    return {
      primary: enable ?? open,
      edit,
      entries: entriesOf([open], [enable, disable], [edit, rename], [share, copy], [del], [cleanup.act({kind:'template',id:template.id}, manage)]),
      keys: keysOf(rename),
      busy: busy(template.id),
    };
  };

  const bulk: BulkAct<Template>[] = [
    {
      id: 'enable',
      label: t('resources.act.enable'),
      icon: <CirclePlay aria-hidden />,
      can: (template) => holds(template.access, 'template.manage') && !template.enabled,
      run: (template) => setEnabled(template, true),
      done: (count) => t('templates.bulk.enabled', { count }),
    },
    {
      id: 'disable',
      label: t('resources.act.disable'),
      icon: <Ban aria-hidden />,
      can: (template) => holds(template.access, 'template.manage') && template.enabled,
      run: (template) => setEnabled(template, false),
      confirm: (rows) => ({
        title: t('templates.bulk.disableTitle', { count: rows.length }),
        body: <BulkNames names={rows.map((row) => row.name)} lead={t('templates.bulk.disableBody')} />,
        label: t('templates.bulk.disableConfirm', { count: rows.length }),
      }),
      done: (count) => t('templates.bulk.disabled', { count }),
    },
    {
      id: 'delete',
      label: t('resources.act.delete'),
      icon: <Trash2 aria-hidden />,
      danger: true,
      can: canDelete,
      run: drop,
      confirm: (rows) => ({
        title: t('templates.bulk.deleteTitle', { count: rows.length }),
        body: <BulkNames names={rows.map((row) => row.name)} lead={t('templates.danger.deleteBody')} />,
        label: t('templates.bulk.deleteConfirm', { count: rows.length }),
      }),
      done: (count) => t('templates.bulk.deleted', { count }),
    },
  ];

  const inUse = remove.error instanceof ApiError && remove.error.is('resource.in_use');
  const dialogs = (
    <>
      {cleanup.dialogs}
      {asked !== null && (asked.what === 'edit' || asked.what === 'rename') && (
        <EditTemplate
          key={`${asked.template.id}-${asked.what}`}
          template={asked.template}
          {...(asked.what === 'rename' ? { only: 'name' as const } : {})}
          onTab={(tab) => toTab(asked.template, tab)}
          onClose={() => setAsked(null)}
        />
      )}
      <ConfirmDialog
        open={asked?.what === 'disable'}
        admin={adminSpec(asked?.template, 'template.manage')}
        onOpenChange={(next) => !next && setAsked(null)}
        title={t('templates.danger.disableConfirmTitle', { name: asked?.template.name ?? '' })}
        confirmLabel={t('templates.danger.disableConfirm', { name: asked?.template.name ?? '' })}
        danger
        pending={switchOff.isPending}
        error={switchOff.error ?? undefined}
        onConfirm={() => asked !== null && switchOff.mutate(asked.template)}
      >
        {t('templates.danger.disableBody', { count: asked?.template.units ?? 0 })}
      </ConfirmDialog>
      <ConfirmDialog
        open={asked?.what === 'delete'}
        admin={adminSpec(asked?.template, 'template.manage')}
        onOpenChange={(next) => {
          if (next) return;
          setAsked(null);
          remove.reset();
        }}
        title={t('templates.danger.deleteConfirmTitle', { name: asked?.template.name ?? '' })}
        confirmLabel={t('templates.danger.deleteConfirm', { name: asked?.template.name ?? '' })}
        danger
        pending={remove.isPending}
        error={remove.error ?? undefined}
        onConfirm={() => asked !== null && remove.mutate(asked.template)}
      >
        <span className="flex flex-col gap-2">
          <span>{t('templates.danger.deleteBody')}</span>
          {inUse && <span>{t('templates.danger.inUse')}</span>}
        </span>
      </ConfirmDialog>
    </>
  );

  return {
    list: { name: (template) => template.name, menuName: (template) => template.name, of: (template) => actsOf(template, 'row'), bulk, dialogs },
    ask: (what, template) => setAsked({ what, template }),
    enable: (template) =>
      track(template.id, async () => {
        const updated = await setEnabled(template, true);
        toast({ tone: 'success', title: t('templates.danger.enabled', { name: updated.name }) });
      }),
    page: (template) => {
      const acts = actsOf(template, 'page');
      return { edit: acts.edit, primary: acts.primary, menu: acts.entries };
    },
    dialogs,
  };
}
