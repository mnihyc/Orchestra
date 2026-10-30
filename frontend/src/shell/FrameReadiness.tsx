/** Frame chrome can render before the single workspace bootstrap admits its data readers. */
import { createContext, type ReactNode, useContext } from 'react';

export const FrameReadiness = createContext<{ ready: boolean; pending: ReactNode }>({ ready: true, pending: null });
export const useFrameReadiness = () => useContext(FrameReadiness);
export const useFrameReady = () => useContext(FrameReadiness).ready;
