/** The words around a match, with the match marked: what a result of a search shows of what was said. */
import { excerptOf } from '@/features/search/hits';
import { useTranslation } from 'react-i18next';

export interface ExcerptProps {
  snippet: string;
  semanticOnly?: boolean;
  /** The ranges of the matches in characters, as the server counts them. */
  matches: readonly (readonly [number, number])[];
}

export function Excerpt({ snippet, matches, semanticOnly = false }: ExcerptProps) {
  const { t } = useTranslation();
  if (semanticOnly && matches.length === 0) return <span className="text-sm text-text-muted">{t('search.similarMeaning')}</span>;
  return (
    <>
      {excerptOf(snippet, matches).map((part, index) =>
        part.marked ? (
          // The parts of a snippet never change while it is shown: their place is their name.
          <mark key={index} className="rounded-sm bg-accent-subtle px-px text-text">
            {part.text}
          </mark>
        ) : (
          <span key={index}>{part.text}</span>
        ),
      )}
    </>
  );
}
