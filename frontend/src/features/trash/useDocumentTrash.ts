/** The one act that moves documents and document folders to the trash, from a menu or from a selection, with its line. */
import { useTranslation } from 'react-i18next';

import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import type { PlaceRef } from '@/features/navigation/places';
import { useErrorText } from '@/ui/errorText';

import { type DocumentTrashResult, restoreDocuments, type TrashEntry, trashDocuments } from './documents';
import { sayTrashed } from './tip';

/** `trash` is the place whose trash Show opens: where the documents lay. */
export function useDocumentTrash(): (entries: readonly TrashEntry[], trash?: PlaceRef | null) => Promise<DocumentTrashResult> {
  const { t } = useTranslation();
  const describe = useErrorText();
  return async (entries, trash = null) => {
    const result = await trashDocuments(entries);
    sayTrashed(t, {
      moved: result.groups.flat().map((entry) => entry.name),
      refused: result.failed.map(({ item, error }) => ({ name: item.name, reason: describe(error).message })),
      ...(trash === null ? {} : { show: () => openNodeDialog({ type: 'deleted', node: trash }) }),
      ...(result.groups.length === 0 ? {} : { undo: () => restoreDocuments(result.groups, (error) => describe(error).message) }),
    });
    return result;
  };
}
