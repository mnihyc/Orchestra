/** File help destinations keep each authored language's heading and are checked with the shipped guide. */
export const FILE_GUIDE = {
  id: 'files-and-attachments',
  sections: {
    previews: { en: 'previews-and-chip-sizes', 'zh-CN': 'previews-and-chip-sizes', term: 'fileMedia.help.previewsTerm', what: 'fileMedia.help.previewsWhat' },
    open: { en: 'open-and-download', 'zh-CN': 'open-and-download', term: 'fileMedia.help.openTerm', what: 'fileMedia.help.openWhat' },
    agent: { en: 'files-for-an-agent', 'zh-CN': 'files-for-an-agent', term: 'fileMedia.help.agentTerm', what: 'fileMedia.help.agentWhat' },
    states: { en: 'preview-states', 'zh-CN': 'preview-states', term: 'fileMedia.help.statesTerm', what: 'fileMedia.help.statesWhat' },
  },
} as const;

export type FileGuideSection = keyof typeof FILE_GUIDE.sections;

export function fileGuideTarget(section: FileGuideSection, language: string) {
  const words = FILE_GUIDE.sections[section];
  return { article: FILE_GUIDE.id, heading: words[language.toLowerCase().startsWith('zh') ? 'zh-CN' : 'en'] };
}
