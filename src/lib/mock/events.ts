/**
 * A minimal in-memory replacement for Tauri's event system, used by the mock
 * backend in browser mode. It deliberately exposes the same shape the IPC
 * layer needs: subscribe by event name, emit by event name.
 */

type Handler<T> = (payload: T) => void;

export interface EventBus {
  /** Subscribe; returns an unlisten function like Tauri's. */
  listen<T>(event: string, handler: Handler<T>): () => void;
  emit<T>(event: string, payload: T): void;
  listenerCount(event: string): number;
}

export function createEventBus(): EventBus {
  const handlers = new Map<string, Set<Handler<unknown>>>();

  return {
    listen(event, handler) {
      let set = handlers.get(event);
      if (!set) {
        set = new Set();
        handlers.set(event, set);
      }
      set.add(handler as Handler<unknown>);
      return () => {
        const current = handlers.get(event);
        current?.delete(handler as Handler<unknown>);
        if (current?.size === 0) handlers.delete(event);
      };
    },
    emit(event, payload) {
      // Copy first: a handler that unlistens during dispatch must not perturb
      // the in-flight iteration.
      const current = handlers.get(event);
      if (!current) return;
      for (const handler of [...current]) {
        handler(payload);
      }
    },
    listenerCount(event) {
      return handlers.get(event)?.size ?? 0;
    }
  };
}
