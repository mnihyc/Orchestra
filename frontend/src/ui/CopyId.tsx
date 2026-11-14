/** The one way an identifier is offered to a person: as a Copy ID act, never as a word in a sentence.
 *
 * A person reads a thing by its name, its kind and its time. Support or an operator sometimes needs the
 * identifier to look the thing up, so it stays one act away, in the thing's details (`CopyId`) or in its ⋯ menu
 * (`copyIdEntry`). Whatever is copied is put on the clipboard as it is and is not shown.
 */
import { Copy } from 'lucide-react';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';

import { copyText } from '@/ui/clipboard';
import { CopyButton } from '@/ui/CopyButton';
import type { MenuEntry } from '@/ui/Menu';

/** The Copy ID act of a thing's details. */
export function CopyId({ value, className }: { value: string; className?: string }) {
  const { t } = useTranslation();
  return <CopyButton value={value} label={t('common.copyId')} {...(className === undefined ? {} : { className })} />;
}

/** The Copy ID entry of a thing's ⋯ menu. */
export function copyIdEntry(t: TFunction, value: string): MenuEntry {
  return { id: 'copy-id', label: t('common.copyId'), icon: <Copy aria-hidden />, onSelect: () => void copyText(value, t('common.idCopied')) };
}
