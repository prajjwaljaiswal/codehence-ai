import React, { useCallback, useEffect, useState } from "react";
import {
  AlertTriangle,
  Check,
  Download,
  FileSpreadsheet,
  Loader2,
  Smartphone,
} from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { api, type TestReportStatus } from "@/lib/api";
import { isTauri } from "@/lib/apiAdapter";

interface TestReportExportProps {
  /** The project whose `.dotsquares-ai/test-report.json` is exported. */
  projectPath: string;
  /**
   * Change this to make the component look again — a run that has just
   * finished is the usual reason a report appears where there was none.
   */
  refreshToken?: unknown;
  /** Matches the size of the buttons it sits beside. */
  size?: "sm" | "default";
  className?: string;
}

/** The sheets the workbook always has, so the dialog can say so before export. */
const SHEETS = [
  "Summary",
  "Test Cases",
  "Mobile Matrix",
  "Defects",
  "Coverage",
  "Environments",
];

/**
 * The URL the web server serves the workbook from.
 *
 * A phone browser has no filesystem for the backend to write into, so on that
 * surface the bytes come back over HTTP and the browser saves them. Desktop
 * goes through a native save dialog instead.
 */
function downloadUrl(projectPath: string): string {
  const url = new URL("/api/test-report/download", window.location.origin);
  url.searchParams.set("project_path", projectPath);
  return url.toString();
}

/**
 * Downloads a Tester run's test documentation as an Excel workbook.
 *
 * The button only appears once a report exists: it is the Tester agent that
 * writes `.dotsquares-ai/test-report.json`, and offering a download for a file no run
 * has produced yet is a dead end rather than a feature.
 */
export const TestReportExport: React.FC<TestReportExportProps> = ({
  projectPath,
  refreshToken,
  size = "default",
  className,
}) => {
  const [status, setStatus] = useState<TestReportStatus | null>(null);
  const [open, setOpen] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [savedTo, setSavedTo] = useState<string | null>(null);
  const [savedTemplateTo, setSavedTemplateTo] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const look = useCallback(async () => {
    if (!projectPath) {
      setStatus(null);
      return;
    }
    try {
      setStatus(await api.testReportStatus(projectPath));
    } catch (err) {
      // Nothing is offered when the check itself fails: the button is an
      // affordance for a file, not a place to report an unrelated error.
      console.warn("[TestReportExport] could not read the test report:", err);
      setStatus(null);
    }
  }, [projectPath]);

  useEffect(() => {
    void look();
  }, [look, refreshToken]);

  // Opening the dialog is the one moment the numbers are certainly being read,
  // so they are re-fetched then rather than trusted from whenever the button
  // last appeared.
  useEffect(() => {
    if (open) void look();
  }, [open, look]);

  const runExport = async () => {
    setExporting(true);
    setError(null);
    setSavedTo(null);
    try {
      if (!isTauri()) {
        // The server sends the bytes; the browser decides where they land.
        window.location.assign(downloadUrl(projectPath));
        return;
      }

      const { save } = await import("@tauri-apps/plugin-dialog");
      const suggested = suggestedName(status);
      const target = await save({
        defaultPath: suggested,
        filters: [{ name: "Excel workbook", extensions: ["xlsx"] }],
      });
      if (typeof target !== "string") return;

      const result = await api.exportTestReport({
        projectPath,
        filePath: target,
      });
      setSavedTo(result.file_path);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setExporting(false);
    }
  };

  const saveTemplate = async () => {
    setError(null);
    setSavedTemplateTo(null);
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const target = await save({
        defaultPath: "test-report.example.json",
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (typeof target !== "string") return;

      await api.saveTestReportTemplate(target);
      setSavedTemplateTo(target);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  if (!status?.found) return null;

  const counts = status.counts;
  const unreadable = Boolean(status.error);

  return (
    <>
      <Button
        variant="outline"
        size={size}
        onClick={() => setOpen(true)}
        className={cn("gap-2", className)}
        title="Download the test documentation as an Excel workbook"
      >
        <FileSpreadsheet className="h-4 w-4" />
        <span>Test doc</span>
        {!unreadable && counts.total > 0 && (
          <span className="text-caption text-muted-foreground tabular-nums">
            {counts.total}
          </span>
        )}
        {unreadable && <AlertTriangle className="h-3.5 w-3.5 text-amber-400" />}
      </Button>

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-[640px]">
          <DialogHeader>
            <DialogTitle>Test documentation</DialogTitle>
            <DialogDescription>
              {unreadable ? (
                <>
                  A report is there but could not be read. Ask the Tester agent
                  to write it again.
                </>
              ) : (
                <>
                  {status.feature || "This project's test report"}
                  {status.generated_at && <> · {status.generated_at}</>} ·
                  exported as one workbook of {SHEETS.length} sheets.
                </>
              )}
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-4 py-2">
            {unreadable ? (
              <div className="rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-400">
                {status.error}
              </div>
            ) : (
              <>
                <div className="grid grid-cols-2 gap-2 sm:grid-cols-3">
                  <Tally label="Test cases" value={counts.total} />
                  <Tally label="Passed" value={counts.passed} tone="pass" />
                  <Tally label="Failed" value={counts.failed} tone="fail" />
                  <Tally
                    label="Blocked"
                    value={counts.blocked}
                    tone={counts.blocked > 0 ? "warn" : undefined}
                  />
                  <Tally label="Not run" value={counts.not_run} />
                  <Tally
                    label="Open defects"
                    value={counts.open_defects}
                    tone={counts.open_defects > 0 ? "fail" : undefined}
                  />
                  <Tally label="Edge cases" value={counts.edge_cases} />
                  <Tally
                    label="Mobile checks"
                    value={counts.mobile_cases}
                    icon={<Smartphone className="h-3 w-3" />}
                    tone={counts.mobile_cases === 0 ? "warn" : undefined}
                  />
                  <Tally
                    label="Pass rate"
                    value={`${Math.round(counts.pass_rate * 1000) / 10}%`}
                  />
                </div>

                {status.problems.length > 0 && (
                  <div className="space-y-1.5 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2">
                    <p className="flex items-center gap-1.5 text-xs font-medium text-amber-400">
                      <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
                      Gaps in this report — they go on the Summary sheet too
                    </p>
                    <ul className="space-y-1 text-xs text-amber-400/90">
                      {status.problems.map((problem) => (
                        <li key={problem} className="pl-5">
                          {problem}
                        </li>
                      ))}
                    </ul>
                  </div>
                )}

                <p className="text-xs text-muted-foreground">
                  Sheets:{" "}
                  <span className="font-mono">{SHEETS.join(" · ")}</span>
                </p>
              </>
            )}

            <p className="truncate text-xs text-muted-foreground">
              Read from <span className="font-mono">{status.path}</span>
            </p>

            <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
              {isTauri() && (
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={saveTemplate}
                  className="h-7 gap-1.5 text-xs"
                >
                  <Download className="h-3.5 w-3.5" />
                  Save an example report
                </Button>
              )}
              {savedTemplateTo && (
                <span className="flex items-center gap-1 text-emerald-400">
                  <Check className="h-3 w-3" />
                  Saved to <span className="font-mono">{savedTemplateTo}</span>
                </span>
              )}
            </div>

            {savedTo && (
              <p className="flex items-center gap-1.5 text-sm text-emerald-400">
                <Check className="h-4 w-4 shrink-0" />
                Saved to <span className="font-mono text-xs">{savedTo}</span>
              </p>
            )}
            {error && <p className="text-sm text-red-400">{error}</p>}
          </div>

          <DialogFooter>
            <Button variant="ghost" onClick={() => setOpen(false)}>
              Close
            </Button>
            <Button onClick={runExport} disabled={exporting || unreadable}>
              {exporting ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : (
                <FileSpreadsheet className="mr-2 h-4 w-4" />
              )}
              Download Excel
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
};

/** A file name that says which feature it is, matching what the backend picks. */
function suggestedName(status: TestReportStatus | null): string {
  const stem = (status?.feature ?? "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 60)
    .replace(/-+$/, "");
  return `${stem || "test-report"}-test-doc.xlsx`;
}

const Tally: React.FC<{
  label: string;
  value: number | string;
  tone?: "pass" | "fail" | "warn";
  icon?: React.ReactNode;
}> = ({ label, value, tone, icon }) => (
  <div className="rounded-lg border border-border/60 px-3 py-2">
    <div className="flex items-center gap-1 text-caption text-muted-foreground">
      {icon}
      {label}
    </div>
    <div
      className={cn(
        "text-lg font-semibold tabular-nums",
        tone === "pass" && "text-emerald-400",
        tone === "fail" && "text-red-400",
        tone === "warn" && "text-amber-400",
      )}
    >
      {value}
    </div>
  </div>
);

export default TestReportExport;
