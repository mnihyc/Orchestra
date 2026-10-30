/** The dialog that edits a template: its name, its description and whether units may be made from it; what it was made as, shown as fixed; and its recipe, changed on its own tab.
 *
 * What a template lets be changed after it was made is what the request takes: a name, a description, a recipe
 * and whether it is on. The driver is chosen when a template is made, and so is whether it is for environments or
 * for providers' runtimes; neither changes afterwards, and the form says so beside each. The recipe is the
 * driver's own JSON, read in full and checked by the server on its tab. Whoever opens it as Rename sees the name alone.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { adminSpec } from '@/ui/AdminAct';

import { driversQuery, templateKey, updateTemplate } from '@/api/resources';
import { EditDialog, Fixed } from '@/features/resources/EditDialog';
import type { TemplateTab } from '@/features/resources/templates/Template';
import type { Template } from '@/generated/api-types';
import { Input } from '@/ui/Input';
import { Switch } from '@/ui/Switch';
import { toast } from '@/ui/Toast';

export interface EditTemplateProps {
  template: Template;
  /** `name` shows the name alone, as Rename does. */
  only?: 'name';
  onTab: (tab: TemplateTab) => void;
  onClose: () => void;
}

export function EditTemplate({ template, only, onClose }: EditTemplateProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const drivers = useQuery(driversQuery);
  const driverTitle = drivers.data?.find((driver) => driver.name === template.driver)?.title;
  const [name, setName] = useState(template.name);
  const [description, setDescription] = useState(template.description);
  const [on, setOn] = useState(template.enabled);
  const nameChanged = name.trim() !== template.name;
  const descriptionChanged = only === undefined && description.trim() !== template.description;
  const onChanged = only === undefined && on !== template.enabled;
  const save = useMutation({
    mutationFn: () =>
      updateTemplate(template.id, {
        expected_revision: template.revision,
        ...(nameChanged ? { name: name.trim() } : {}),
        ...(descriptionChanged ? { description: description.trim() } : {}),
        ...(onChanged ? { enabled: on } : {}),
      }),
    onSuccess: (updated) => {
      toast({ tone: 'success', title: t('resources.edit.saved', { name: updated.name }) });
      onClose();
    },
    // Somebody else changed it first: it is read again before the next try.
    onError: () => void queryClient.invalidateQueries({ queryKey: templateKey(template.id) }),
  });

  return (
    <EditDialog
      admin={adminSpec(template, 'template.manage')}
      title={only === 'name' ? t('resources.edit.renameTitle', { name: template.name }) : t('resources.edit.title', { name: template.name })}
      changed={nameChanged || descriptionChanged || onChanged}
      valid={name.trim() !== ''}
      pending={save.isPending}
      error={save.error}
      onSave={() => save.mutate()}
      onClose={onClose}
      {...(only === 'name'
        ? {}
        : {
            rest: (
              <>
                <Fixed label={t('templates.overview.kind')} value={t(`templates.kind.${template.kind}`)} why={t('templates.edit.kindWhy')} />
                <Fixed label={t('templates.overview.driver')} value={driverTitle === undefined ? template.driver : `${driverTitle} (${template.driver})`} why={t('templates.edit.driverWhy')} />
              </>
            ),
          })}
    >
      <Input label={t('resources.edit.name')} value={name} disabled={save.isPending} onChange={(event) => setName(event.target.value)} autoComplete="off" />
      {only === undefined && (
        <>
          <Input label={t('resources.edit.description')} optional value={description} disabled={save.isPending} onChange={(event) => setDescription(event.target.value)} autoComplete="off" />
          <Switch name={t('templates.modeChoices.enabled.name')} description={t('templates.modeChoices.enabled.description')} checked={on} disabled={save.isPending} onCheckedChange={setOn} />
        </>
      )}
    </EditDialog>
  );
}
