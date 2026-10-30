/** The field that narrows a tree as its reader types: what has the typed letters in its name stays, with the way to it.
 *
 * It is drawn as the search fields of the dialogs are, and is the same field wherever a tree is
 * narrowed. Escape empties it and gives the keyboard back to the tree, and the arrow down goes on
 * into what it found.
 */
import { Search, X } from 'lucide-react';
import type { Ref } from 'react';
import { useTranslation } from 'react-i18next';

import { IconButton } from '@/ui/IconButton';
import { fieldBoxClasses } from '@/ui/Input';

export interface TreeFilterProps {
  short?: boolean;
  value: string;
  onChange: (value: string) => void;
  /** Leaves the field: Escape, after it has emptied what was typed. */
  onLeave: () => void;
  /** Goes on into the rows that were found: the arrow down, and Enter. */
  onDown: () => void;
  ref?: Ref<HTMLInputElement>;
}

export function TreeFilter({ value, onChange, onLeave, onDown, short = false, ref }: TreeFilterProps) {
  const { t } = useTranslation();
  return (
    <div className="relative flex items-center px-2 py-1">
      <label className="relative block min-w-0 flex-1">
        <span className="sr-only">{t('navigation.filter.label')}</span>
        <Search aria-hidden size={16} className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-text-subtle" />
        <input
          ref={ref}
          type="search"
          value={value}
          autoComplete="off"
          placeholder={t(short ? 'navigation.filter.recentPlaceholder' : 'navigation.filter.placeholder')}
          onChange={(event) => onChange(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Escape') {
              event.preventDefault();
              event.stopPropagation();
              onChange('');
              onLeave();
            } else if (event.key === 'ArrowDown' || event.key === 'Enter') {
              event.preventDefault();
              onDown();
            }
          }}
          // The browser's own cross of a search field is left out: the button at the end empties the field.
          className={`${fieldBoxClasses} input-leading-icon ${value === '' ? '' : 'input-trailing-action'} [&::-webkit-search-cancel-button]:hidden`}
        />
      </label>
      {value !== '' && (
        <span className="absolute inset-y-0 right-3 flex items-center">
          <IconButton size="sm" label={t('navigation.filter.clear')} icon={<X aria-hidden />} onClick={() => onChange('')} />
        </span>
      )}
    </div>
  );
}
