/**
 * Unified event listening for Tauri (desktop) and web (browser) modes.
 *
 * Desktop: the Rust side emits real Tauri events (`claude-output`, etc.), which
 * are delivered through `@tauri-apps/api/event`.
 * Web: `apiAdapter` re-dispatches WebSocket frames as DOM CustomEvents of the
 * same name, so we listen on `window` instead.
 *
 * The Tauri module is imported statically on purpose. A conditional
 * `require(...)` does not work here: the app is bundled as ESM, so `require`
 * is not defined at runtime and the import silently fails. Importing the
 * module is safe in web mode - it only touches Tauri internals when called.
 */
import { listen as tauriListen } from "@tauri-apps/api/event";
import { isTauri } from "./apiAdapter";

export type UnlistenFn = () => void;

export interface EventPayload<T = any> {
  payload: T;
}

export async function listen<T = any>(
  eventName: string,
  callback: (event: EventPayload<T>) => void
): Promise<UnlistenFn> {
  if (isTauri()) {
    return (await tauriListen<T>(eventName, callback as any)) as UnlistenFn;
  }

  const handler = (e: Event) => callback({ payload: (e as CustomEvent).detail });
  window.addEventListener(eventName, handler);
  return () => window.removeEventListener(eventName, handler);
}
