/** Draw the small shell first; application services arrive after an actual painted frame. */
import './styles/base.css';
import { createRoot } from 'react-dom/client';
import { FirstShell } from '@/shell/FirstShell';
import { applyDeviceAppearance, readDeviceAppearance } from '@/lib/deviceAppearance';
import { mark } from '@/lib/marks';

mark('start/script');
const appearance = readDeviceAppearance();
applyDeviceAppearance(appearance);
const cover = document.createElement('div');
document.body.append(cover);
createRoot(cover).render(<FirstShell locale={appearance.locale} load={() => import('@/application')} />);
