/** A part of the application fetched when it is first drawn, three times over if need be, and made anew when it is asked for again after it failed.
 *
 * A viewer, the editor and a view of the workbench each arrive as a chunk of their own. A line that
 * fails once is not the end of them: the fetch is tried again, and when it fails for good the view that
 * needed it shows the failure with "Try again", which draws it anew and so fetches it anew.
 */
import { type ComponentType, lazy } from 'react';

/** Fetches a part up to three times, a short pause between the tries. */
async function loadThrice<T>(load: () => Promise<T>, left = 3): Promise<T> {
  try {
    return await load();
  } catch (error) {
    if (left <= 1) throw error;
    await new Promise((resolve) => setTimeout(resolve, 400));
    return loadThrice(load, left - 1);
  }
}

/** A component fetched when first drawn. One that failed to arrive is fetched again the next time it is drawn. */
export function lazyPart<Props extends object>(load: () => Promise<{ default: ComponentType<Props> }>): ComponentType<Props> {
  const make = () =>
    lazy(() =>
      loadThrice(load).catch((error: unknown) => {
        current = make();
        throw error;
      }),
    );
  let current = make();
  return function LazyPart(props: Props) {
    const Inner = current;
    return <Inner {...(props as Props & React.JSX.IntrinsicAttributes)} />;
  };
}
