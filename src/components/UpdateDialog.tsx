import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { useAppUpdater } from "@/hooks/useAppUpdater";
import { Loader2, Download } from "lucide-react";

/**
 * Shows a one-time prompt when a newer version is published to GitHub
 * Releases. The user chooses when to install - nothing happens silently.
 */
export function UpdateDialog() {
  const { status, version, releaseNotes, progress, installUpdate, dismiss } = useAppUpdater();

  const open = status === "available" || status === "downloading";

  return (
    <Dialog open={open} onOpenChange={(next) => !next && status === "available" && dismiss()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Update available{version ? ` — v${version}` : ""}</DialogTitle>
          <DialogDescription>
            {status === "downloading"
              ? "Downloading the update. The app will restart automatically once it's installed."
              : "A new version is ready to install. Restart now to update, or install it later."}
          </DialogDescription>
        </DialogHeader>

        {releaseNotes && status === "available" && (
          <div className="max-h-40 overflow-y-auto rounded-md border bg-muted/40 p-3 text-sm whitespace-pre-wrap">
            {releaseNotes}
          </div>
        )}

        {status === "downloading" && (
          <div className="w-full">
            <div className="h-2 w-full overflow-hidden rounded-full bg-muted">
              <div
                className="h-full bg-primary transition-all"
                style={{ width: progress !== null ? `${progress}%` : "40%" }}
              />
            </div>
          </div>
        )}

        <DialogFooter>
          {status === "available" && (
            <>
              <Button variant="outline" onClick={dismiss}>
                Later
              </Button>
              <Button onClick={installUpdate}>
                <Download className="mr-2 h-4 w-4" />
                Restart &amp; Install
              </Button>
            </>
          )}
          {status === "downloading" && (
            <Button disabled>
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              Installing…
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
