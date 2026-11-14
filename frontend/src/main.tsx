/** Static application entry point.
 *
 * Starts the things everything else stands on: how the page looks (so that nothing is drawn in the wrong theme), the
 * session and the code of the screen the address names (asked for at once, beside the words, since they need neither words
 * nor a first draw), the catalogue of its reader's language (so that nothing is drawn without its words), and then the
 * application. What starts at the session's answer is said in `shell/startup.ts`.
 */
import './styles/base.css';

import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import { App, router } from '@/app';
import { client } from '@/api/client';
import { startI18n } from '@/i18n';
import { startAppearance } from '@/lib/appearance';
import { mark } from '@/lib/marks';
import { KeptShellCover } from '@/shell/KeptShellCover';
import { beginStartup } from '@/shell/startup';

mark('start/script');
const appearance = startAppearance();
// A screen for somebody signed in stands in the workbench, whose code is a part of its own: it is asked for now, beside
// the words, rather than after them. The screens before sign-in never fetch it.
const [, , found] = router.getMatchedRoutes(location.pathname);
const signedInScreen = (found?.options.staticData?.needs ?? 'signed_in') === 'signed_in';
if (signedInScreen) void import('@/shell/Shell');
let readerLocale = appearance.locale;
const stopLocale = client.onSession((event) => { if (event.type === 'signed_in') readerLocale = event.result.user.locale; });
beginStartup(router, location.href, signedInScreen);
// Anonymous frame geometry can draw before the session; private names wait for its confirmed reader.
if (signedInScreen) {
  const cover = document.createElement('div');
  document.body.append(cover);
  createRoot(cover).render(<KeptShellCover />);
}
if (signedInScreen) await client.restore().catch(() => undefined);
stopLocale();
await startI18n(readerLocale);
mark('start/words');

const root = document.getElementById('root');
if (root === null) throw new Error('The page has no element to draw the application in');

mark('start/render');
createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
