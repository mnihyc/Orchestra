/** Existing named groups edit the same option declarations as their endpoint and model. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { providerKey, updateProvider } from '@/api/resources';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { EditDialog } from '@/features/resources/EditDialog';
import { requestOptions } from '@/features/resources/providers/entry';
import type { Provider, ProviderRef } from '@/generated/api-types';
import { adminSpec } from '@/ui/AdminAct';
import { Textarea } from '@/ui/Textarea';
import { toast } from '@/ui/Toast';

export function GroupOptionsDialog({ provider, revision, group, options, onClose }: {
  provider: ProviderRef;
  revision: number;
  group: string;
  options: NonNullable<Provider['model_group_options']>;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const original = options[group]?.request_options ?? {};
  const [text, setText] = useState(() => JSON.stringify(original, null, 2));
  const parsed = requestOptions(text);
  const value = parsed ?? {};
  const valid = text.trim() === '' || parsed !== undefined;
  const changed = JSON.stringify(value) !== JSON.stringify(original);
  const save = useMutation({
    mutationFn: () => updateProvider(provider.id, {
      expected_revision: revision,
      model_group_options: { ...options, [group]: { ...options[group], request_options: value } },
    }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: providerKey(provider.id) });
      toast({ tone: 'success', title: t('models.groups.optionsSaved', { name: group }) });
      onClose();
    },
    onError: () => void queryClient.invalidateQueries({ queryKey: providerKey(provider.id) }),
  });
  return <EditDialog admin={adminSpec(provider, 'provider.manage')} title={t('models.groups.optionsTitle', { name: group })}
    changed={changed} valid={valid} pending={save.isPending} error={save.error} onSave={() => save.mutate()} onClose={onClose}>
    <div className="flex justify-end"><ArticleGuideLink article="provider-details" heading="request-options" title={t('models.groups.optionsGuide')} /></div>
    <Textarea label={t('providerOptions.schemaTitle')} rows={7} className="font-mono" value={text} disabled={save.isPending}
      error={valid ? undefined : t('providerOptions.invalidSchema')} onChange={(event) => setText(event.target.value)} />
  </EditDialog>;
}
