/** A setting supplies the catalogue context while nested schema controls remain independent of setting keys. */
import { createContext } from 'react';

export const SettingChoiceContext = createContext<string | null>(null);
