/** Goes to a place of the application from inside the Links section.
 *
 * On a phone the section stands in a sheet over the page, and a jump to a mention or to the full graph is a jump to the
 * page: the sheet gives way to it, as the Outline's does.
 */
import { useCallback } from 'react';

import { client } from '@/api/client';
import { useOpenPath } from '@/features/chat/open';
import { useShape } from '@/features/workbench/Narrow';
import { writeViewChoice } from '@/lib/viewChoices';

export function useOpenLinked(): (path: string) => void {
  const open = useOpenPath();
  const phone = useShape() === 'small';
  return useCallback((path: string) => {
    open(path);
    if (!phone) return;
    const user = client.session.getState().userId;
    for (const key of ['mobileView', 'mobileContainer']) writeViewChoice(user, 'global', key, '');
  }, [open, phone]);
}
