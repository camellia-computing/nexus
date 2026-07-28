export type Unlisten = () => void;

export type AsyncListenerScope = {
  active: () => boolean;
  track: (registration: Promise<Unlisten>) => void;
  dispose: () => void;
};

export function createAsyncListenerScope(
  reportError: (error: unknown) => void,
): AsyncListenerScope {
  let disposed = false;
  const unlisteners = new Set<Unlisten>();

  const release = (unlisten: Unlisten) => {
    try {
      unlisten();
    } catch (error) {
      reportError(error);
    }
  };

  return {
    active: () => !disposed,
    track(registration) {
      void registration.then((unlisten) => {
        if (disposed) {
          release(unlisten);
          return;
        }
        unlisteners.add(unlisten);
      }).catch((error) => {
        if (!disposed) reportError(error);
      });
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      for (const unlisten of unlisteners) release(unlisten);
      unlisteners.clear();
    },
  };
}
