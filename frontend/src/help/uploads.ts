/** Each upload question leads to its authored section; a section has one id in every language (`heading_id` in the Chinese edition). */
export const UPLOAD_HELP = {
  'message-files': { article: 'paste-and-upload', heading: 'message-files', title: 'uploadHelp.messageTitle', what: 'uploadHelp.messageWhat' },
  'pasted-text': { article: 'paste-and-upload', heading: 'pasted-text', title: 'uploads.pastedText', what: 'uploadHelp.pasteWhat' },
  'workspace-uploads': { article: 'paste-and-upload', heading: 'workspace-uploads', title: 'uploadHelp.filesTitle', what: 'uploadHelp.filesWhat' },
  documents: { article: 'paste-and-upload', heading: 'document-files', title: 'uploadHelp.documentTitle', what: 'uploadHelp.documentWhat' },
} as const;

/** The queue article's section on putting an untaken steer back: one id in every language. */
export const QUEUE_HELP = { article: 'message-queue', heading: '5-put-an-untaken-steer-back' } as const;

export type UploadHelpTopic = keyof typeof UPLOAD_HELP;
export function uploadHelpHeading(topic: UploadHelpTopic): string {
  return UPLOAD_HELP[topic].heading;
}
