/** Resource menus open the same scoped cleanup plan used by Storage. */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { BrushCleaning } from 'lucide-react';
import type { CleanupRequest } from '@/generated/api-types';
import type { Act } from '@/features/resources/acts';
import { Cleanup } from './Cleanup';

type ResourceObject = NonNullable<CleanupRequest['resource']>;

export function useResourceCleanup() {
  const { t } = useTranslation();
  const [resource, setResource] = useState<ResourceObject | null>(null);
  return {
    act: (object: ResourceObject, manageable: boolean): Act => ({ id: 'cleanup', label: t('storage.clean'), icon: <BrushCleaning aria-hidden />, disabled: manageable ? false : t('storage.cleanup.manageRequired'), run: () => setResource(object) }),
    dialogs: resource === null ? null : <Cleanup request={{ scope: 'me', resource, items: [] }} onClose={() => setResource(null)} />,
  };
}
