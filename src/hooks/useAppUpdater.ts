import { useCallback, useEffect, useRef, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export type UpdateStatus = "idle" | "checking" | "available" | "downloading" | "ready" | "error";

interface UseAppUpdaterResult {
  status: UpdateStatus;
  version: string | null;
  releaseNotes: string | null;
  progress: number | null;
  error: string | null;
  installUpdate: () => Promise<void>;
  dismiss: () => void;
}

/**
 * Checks the configured updater endpoint (GitHub Releases' latest.json) once
 * on mount. Never installs automatically - the caller decides when to call
 * installUpdate() (e.g. after the user confirms in a dialog).
 */
export function useAppUpdater(): UseAppUpdaterResult {
  const [status, setStatus] = useState<UpdateStatus>("idle");
  const [version, setVersion] = useState<string | null>(null);
  const [releaseNotes, setReleaseNotes] = useState<string | null>(null);
  const [progress, setProgress] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const updateRef = useRef<Update | null>(null);

  useEffect(() => {
    let cancelled = false;

    (async () => {
      setStatus("checking");
      try {
        const update = await check();
        if (cancelled) return;

        if (update?.available) {
          updateRef.current = update;
          setVersion(update.version);
          setReleaseNotes(update.body ?? null);
          setStatus("available");
        } else {
          setStatus("idle");
        }
      } catch (err) {
        if (cancelled) return;
        // Most common cause: no network, or no published release yet.
        // Fail quietly - this must never block app startup.
        console.error("Update check failed:", err);
        setError(err instanceof Error ? err.message : String(err));
        setStatus("error");
      }
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  const installUpdate = useCallback(async () => {
    const update = updateRef.current;
    if (!update) return;

    setStatus("downloading");
    setProgress(0);
    let downloaded = 0;
    let total: number | null = null;

    try {
      await update.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            total = event.data.contentLength ?? null;
            break;
          case "Progress":
            downloaded += event.data.chunkLength;
            setProgress(total ? Math.min(100, Math.round((downloaded / total) * 100)) : null);
            break;
          case "Finished":
            setProgress(100);
            break;
        }
      });

      setStatus("ready");
      await relaunch();
    } catch (err) {
      console.error("Update install failed:", err);
      setError(err instanceof Error ? err.message : String(err));
      setStatus("error");
    }
  }, []);

  const dismiss = useCallback(() => {
    setStatus("idle");
  }, []);

  return { status, version, releaseNotes, progress, error, installUpdate, dismiss };
}
