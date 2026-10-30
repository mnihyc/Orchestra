/** Static application entry point.
 *
 * Starts the three things everything else stands on, in the order they are needed: how the page
 * looks (so that nothing is drawn in the wrong theme), the catalogue of its reader's language (so
 * that nothing is drawn without its words), and then the application.
 */
import './styles/base.css';

import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import { App, router } from '@/app';
import { startI18n } from '@/i18n';
import { startAppearance } from '@/lib/appearance';

const appearance = startAppearance();
// A screen for somebody signed in stands in the workbench, whose code is a part of its own: it is asked for now, beside
// the words, rather than after them. The screens before sign-in never fetch it.
const [, , found] = router.getMatchedRoutes(location.pathname);
if ((found?.options.staticData?.needs ?? 'signed_in') === 'signed_in') void import('@/shell/Shell');
await startI18n(appearance.locale);

const root = document.getElementById('root');
if (root === null) throw new Error('The page has no element to draw the application in');

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
