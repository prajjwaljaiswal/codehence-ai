import React, { useState } from "react";
import { Loader2, FileSpreadsheet, AlertTriangle, Check, Download } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { api, type ImportPreview } from "@/lib/api";

/** Workbook formats calamine reads, plus the delimited text ones. */
const SPREADSHEET_EXTENSIONS = [
  "xlsx",
  "xlsm",
  "xls",
  "xlsb",
  "ods",
  "csv",
  "tsv",
  "tab",
];

interface ImportTicketsDialogProps {
  projectId: number;
  onClose: () => void;
  /** Called once tickets exist, with how many were created. */
  onImported: (created: number) => void;
}

/**
 * Creates many tickets from a spreadsheet.
 *
 * The file is shown before anything is created: a sheet of thirty rows with a
 * misspelt heading or one bad priority should say so while it can still be
 * fixed, not after half a board has appeared.
 */
export const ImportTicketsDialog: React.FC<ImportTicketsDialogProps> = ({
  projectId,
  onClose,
  onImported,
}) => {
  const [filePath, setFilePath] = useState<string | null>(null);
  const [preview, setPreview] = useState<ImportPreview | null>(null);
  const [loading, setLoading] = useState(false);
  const [importing, setImporting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedTemplateTo, setSavedTemplateTo] = useState<string | null>(null);

  const pickFile = async () => {
    setError(null);
    try {
      // The dialog plugin only exists in the desktop shell.
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        multiple: false,
        filters: [{ name: "Spreadsheet or CSV", extensions: SPREADSHEET_EXTENSIONS }],
      });
      if (typeof selected !== "string") return;

      setFilePath(selected);
      setPreview(null);
      setLoading(true);
      setPreview(await api.previewTicketImport(selected));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  /**
   * Hand over an example sheet to start from.
   *
   * Knowing which headings the importer understands is otherwise something you
   * find out by guessing at one, so the answer is a file you can open.
   */
  const saveTemplate = async () => {
    setError(null);
    setSavedTemplateTo(null);
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const target = await save({
        defaultPath: "tickets-template.csv",
        filters: [{ name: "CSV", extensions: ["csv"] }],
      });
      if (typeof target !== "string") return;

      await api.saveTicketTemplate(target);
      setSavedTemplateTo(target);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const runImport = async () => {
    if (!filePath) return;
    setImporting(true);
    setError(null);
    try {
      const result = await api.importTickets({ projectId, filePath });
      onImported(result.created);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setImporting(false);
    }
  };

  const usable = preview?.rows.filter((r) => !r.problem) ?? [];
  const problems = preview?.rows.filter((r) => r.problem) ?? [];

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-[720px]">
        <DialogHeader>
          <DialogTitle>Import tickets</DialogTitle>
          <DialogDescription>
            One ticket per row. The first non-empty row is read as headings:{" "}
            <span className="font-mono text-xs">title</span> is required;{" "}
            <span className="font-mono text-xs">epic</span>,{" "}
            <span className="font-mono text-xs">description</span>,{" "}
            <span className="font-mono text-xs">priority</span> are optional. The
            agent for each ticket is chosen from what it asks for.
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-2">
          <div className="flex items-center gap-3">
            <Button variant="outline" onClick={pickFile} className="gap-2">
              <FileSpreadsheet className="h-4 w-4" />
              Choose file
            </Button>
            {filePath && (
              <span className="truncate font-mono text-xs text-muted-foreground">
                {filePath}
              </span>
            )}
            {loading && <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />}
          </div>

          {!preview && (
            <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
              <span>Not sure of the columns?</span>
              <Button variant="ghost" size="sm" onClick={saveTemplate} className="h-7 gap-1.5 text-xs">
                <Download className="h-3.5 w-3.5" />
                Save an example sheet
              </Button>
              {savedTemplateTo && (
                <span className="flex items-center gap-1 text-emerald-400">
                  <Check className="h-3 w-3" />
                  Saved to <span className="font-mono">{savedTemplateTo}</span>
                </span>
              )}
            </div>
          )}

          {preview && (
            <>
              <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
                <span>
                  Sheet <span className="font-mono">{preview.sheet}</span>
                </span>
                <span className="flex items-center gap-1 text-emerald-400">
                  <Check className="h-3 w-3" />
                  {usable.length} ready
                </span>
                {problems.length > 0 && (
                  <span className="flex items-center gap-1 text-amber-400">
                    <AlertTriangle className="h-3 w-3" />
                    {problems.length} will be skipped
                  </span>
                )}
              </div>

              {preview.ignored_columns.length > 0 && (
                <div className="rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-400">
                  Not understood, so these columns were not read:{" "}
                  <span className="font-mono">{preview.ignored_columns.join(", ")}</span>
                </div>
              )}

              <div className="max-h-[320px] overflow-auto rounded-lg border border-border/60">
                <table className="w-full text-left text-xs">
                  <thead className="sticky top-0 bg-muted/80 backdrop-blur">
                    <tr className="text-muted-foreground">
                      <th className="px-2 py-1.5 font-medium">Row</th>
                      <th className="px-2 py-1.5 font-medium">Title</th>
                      <th className="px-2 py-1.5 font-medium">Epic</th>
                      <th className="px-2 py-1.5 font-medium">Priority</th>
                      <th className="px-2 py-1.5 font-medium">Description</th>
                    </tr>
                  </thead>
                  <tbody>
                    {preview.rows.map((r) => (
                      <tr
                        key={r.row}
                        className={cn(
                          "border-t border-border/40",
                          r.problem && "bg-amber-500/5 text-muted-foreground"
                        )}
                      >
                        <td className="px-2 py-1.5 tabular-nums text-muted-foreground">
                          {r.row}
                        </td>
                        <td className="px-2 py-1.5">
                          {r.problem ? (
                            <span className="flex items-center gap-1.5 text-amber-400">
                              <AlertTriangle className="h-3 w-3 shrink-0" />
                              {r.problem}
                            </span>
                          ) : (
                            r.title
                          )}
                        </td>
                        <td className="px-2 py-1.5">{r.epic ?? "—"}</td>
                        <td className="px-2 py-1.5">{r.priority}</td>
                        <td className="max-w-[260px] truncate px-2 py-1.5 text-muted-foreground">
                          {r.description ?? "—"}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>

            </>
          )}

          {error && <p className="text-sm text-red-400">{error}</p>}
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button onClick={runImport} disabled={importing || usable.length === 0}>
            {importing && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            Import {usable.length || ""} ticket{usable.length === 1 ? "" : "s"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default ImportTicketsDialog;
