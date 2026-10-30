/** The reading guide's authored headings are shared by controls and the shipped-content check. */
export const READING_GUIDE = {
  id: 'reading-views-and-details',
  title: 'readingGuide.title',
  headings: {
    en: ['opening-a-screen', 'views-and-navigation', 'cards-and-menus', 'values-and-settings', 'reading-documents-and-files', 'finding-names-and-commands'],
    'zh-CN': ['opening-a-screen', 'views-and-navigation', 'cards-and-menus', 'values-and-settings', 'reading-documents-and-files', 'finding-names-and-commands'],
  },
} as const;

export function readingGuideHeading(language: string, section: 'views' | 'cards' | 'finder'): string {
  const index = { views: 1, cards: 2, finder: 5 } as const;
  return READING_GUIDE.headings[language.split('-')[0] === 'zh' ? 'zh-CN' : 'en'][index[section]];
}
