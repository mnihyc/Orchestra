/** Reference sizes follow the place they appear and stay with the reader's workbench unless the document owns the choice. */
import { createContext, useContext } from 'react';
import { useWorkbench } from '@/features/workbench/store';
import type { MenuEntry } from '@/ui/Menu';
import type { TFunction } from 'i18next';
import { Check } from 'lucide-react';
import { createElement } from 'react';

export type ChipSize = 'small' | 'medium' | 'large';
export type ChipPlace = 'compact' | 'prose' | 'document';
export type ChipSizes = Partial<Record<ChipPlace, ChipSize>>;
export const ChipPlaceContext = createContext<ChipPlace>('prose');

export const isChipSize = (value: unknown): value is ChipSize => value === 'small' || value === 'medium' || value === 'large';
export function readChipSizes(value: unknown): ChipSizes {
  if (typeof value !== 'object' || value === null) return {};
  return Object.fromEntries(Object.entries(value).filter(([place, size]) => ['compact', 'prose', 'document'].includes(place) && isChipSize(size)));
}
export function chipSize(place: ChipPlace, chosen?: ChipSize): ChipSize {
  const size = chosen ?? (place === 'compact' ? 'small' : place === 'document' ? 'large' : 'medium');
  return size === 'large' && place === 'compact' ? 'medium' : size;
}
export interface ChipPresentation {
  place?: ChipPlace | undefined;
  size?: ChipSize | undefined;
  /** A document owns this choice and writes it through its existing edit transaction. */
  onSizeChange?: ((size: ChipSize) => void) | undefined;
  /** Read-only documents use the stored choice without changing the reader's preferences. */
  document?: boolean | undefined;
}

export function useChipPresentation(presentation: ChipPresentation = {}) {
  const context = useContext(ChipPlaceContext);
  const place = presentation.place ?? context;
  const saved = useWorkbench((state) => state.chipSizes[place]);
  const change = useWorkbench((state) => state.setChipSize);
  const chosen = presentation.size ?? (presentation.document === true ? undefined : saved);
  return { size: chipSize(place, chosen), chosen: chosen ?? chipSize(place),
    change: presentation.onSizeChange ?? (presentation.document === true ? undefined : (size: ChipSize) => change(place, size)) };
}

export function chipSizeMenu(t: TFunction, chosen: ChipSize, change?: (size: ChipSize) => void): MenuEntry[] {
  if (change === undefined) return [{ type: 'sub', id: 'show-as', label: t('fileMedia.showAs'), entries:
    (['small', 'medium', 'large'] as const).map((size) => ({ id: `size-${size}`, label: t(`fileMedia.sizes.${size}`),
      ...(chosen === size ? { icon: createElement(Check, { 'aria-hidden': true }) } : {}), disabled: t('fileMedia.sizeNeedsEdit'), onSelect: () => undefined })) }];
  return [{ type: 'sub', id: 'show-as', label: t('fileMedia.showAs'), entries: [{ type: 'radio', id: 'chip-size', label: t('fileMedia.showAs'), value: chosen,
    onValueChange: (size) => { if (isChipSize(size)) change(size); },
    options: (['small', 'medium', 'large'] as const).map((size) => ({ value: size, label: t(`fileMedia.sizes.${size}`) })) }] }];
}
