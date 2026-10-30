/** Creates a document beside a node and opens its title with the caret: one press, then write on the page itself. */
import { useMutation } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';

import { createDocument } from '@/api/documents';
import type { Access } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { newActKey } from '@/lib/requests';
import { toast } from '@/ui/Toast';

/** What a new document begins from: a title (the untitled one when absent), the Markdown it is made of, and whether only its maker may open it. */
export interface NewDocument {
  title?: string;
  markdown?: string;
  private?: boolean;
}

/** The act of making a document beside `node`. One press makes one document, however often the answer is waited for. */
export function useMakeDocument(node: { id: string; access?: Pick<Access, 'permissions'> }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  return useMutation({
    mutationFn: (from: NewDocument) =>
      createDocument(
        node.id,
        {
          title: from.title ?? t('documents.page.untitled'),
          ...(from.markdown === undefined ? {} : { markdown: from.markdown }),
          access: from.private === true ? 'private' : 'follows',
        },
        newActKey(),
      ),
    onSuccess: (made) => {
      // A person who may not write the others' documents is told, once, that this one is theirs.
      if (node.access !== undefined && !holds(node.access, 'doc.edit')) toast({ tone: 'info', title: t('documents.list.own') });
      void navigate({ to: '/doc/$documentId', params: { documentId: made.id }, search: { new: true } });
    },
  });
}
