/** A value that follows another after it has stopped changing for a moment: what a search asks the server with while its reader is still typing. */
import { useEffect, useState } from 'react';

/**
 * How long a field rests before what was typed in it is asked for: long enough that the keys of one
 * word are one question, short enough that the answer seems to follow the typing.
 */
export const TYPING_PAUSE_MS = 250;

export function useDebounced<Value>(value: Value, pauseMs: number = TYPING_PAUSE_MS): Value {
  const [settled, setSettled] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setSettled(value), pauseMs);
    return () => clearTimeout(timer);
  }, [value, pauseMs]);
  return settled;
}
