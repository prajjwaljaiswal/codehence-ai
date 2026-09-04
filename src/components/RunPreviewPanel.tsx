import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { motion } from "framer-motion";
import {
  X,
  RefreshCw,
  Loader2,
  Check,
  AlertTriangle,
  GitBranch,
  GitPullRequest,
  ExternalLink,
  FileDiff,
  Terminal,
  Square,
  HelpCircle,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import {
  api,
  type RunDiff,
  type Ticket,
  type WorkflowEvent,
  type WorkflowPhase,
  type WorkflowRun,
} from "@/lib/api";
import { listen } from "@/lib/events";
import { StreamMessage } from "./StreamMessage";
import type { ClaudeStreamMessage } from "./AgentExecution";

/** The phases a run walks through, in order, as the stepper shows them. */
const PHASES: { phase: WorkflowPhase; label: string }[] = [
  { phase: "preflight", label: "Preflight" },
  { phase: "branch", label: "Branch" },
  { phase: "agent", label: "Implement" },
  { phase: "test", label: "Test" },
  { phase: "commit", label: "Commit" },
  { phase: "merge", label: "Merge" },
  { phase: "push", label: "Push" },
  { phase: "pr", label: "PR" },
];

/** How often the diff is re-fetched while a run is still working. */
const LIVE_DIFF_INTERVAL_MS = 8000;

/**
 * Colour one line of a unified diff.
 *
 * `+++`/`---` are checked before `+`/`-` so file headers do not get painted as
 * additions and deletions.
 */
function diffLineClass(line: string): string {
  if (line.startsWith("+++") || line.startsWith("---")) return "text-muted-foreground";
  if (line.startsWith("diff ") || line.startsWith("index ")) return "text-muted-foreground";
  if (line.startsWith("@@")) return "text-cyan-400";
  if (line.startsWith("+")) return "text-emerald-400";
  if (line.startsWith("-")) return "text-red-400";
  return "text-foreground/70";
}

interface RunPreviewPanelProps {
  ticket: Ticket;
  onClose: () => void;
}

/**
 * Live view of the run behind a ticket: how far it has got, what the agent is
 * saying, and the diff it has produced so far.
 *
 * While a run is in flight the diff is taken against the working tree, so the
 * agent's edits are visible before anything is committed - the whole point of
 * watching rather than waiting.
 */
export const RunPreviewPanel: React.FC<RunPreviewPanelProps> = ({ ticket, onClose }) => {
  const runId = ticket.workflow_run_id;

  const [messages, setMessages] = useState<ClaudeStreamMessage[]>([]);
  const [events, setEvents] = useState<WorkflowEvent[]>([]);
  const [tab, setTab] = useState<"output" | "diff">("output");
  const [diff, setDiff] = useState<RunDiff | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);
  const [diffError, setDiffError] = useState<string | null>(null);
  const [run, setRun] = useState<WorkflowRun | null>(null);
  const [stopping, setStopping] = useState(false);
  const [stopError, setStopError] = useState<string | null>(null);

  const outputEnd = useRef<HTMLDivElement>(null);

  const latest = events[events.length - 1] ?? null;

  // Events only arrive while this panel is open, so a run already under way
  // when it opens has none. The run's own record of where it got to stands in.
  const phase: WorkflowPhase | null = latest?.phase ?? run?.phase ?? null;

  // Live events exist only while a run is in flight; the stored row is what a
  // finished or failed run has to be read from, and is the truth either way.
  const running = run ? run.status === "running" : ticket.status === "in_progress";
  const cancelled = phase ? phase === "cancelled" : run?.status === "cancelled";
  // A waiting run is still going; it just cannot get on until someone answers.
  const waiting = phase === "waiting";
  const failed = cancelled
    ? false
    : latest
      ? latest.phase === "failed" || latest.ok === false
      : run?.status === "failed" || phase === "failed";
  const statusLine = latest?.message ?? run?.error ?? run?.phase_message ?? null;

  const stopRun = async () => {
    if (runId === null) return;
    setStopping(true);
    setStopError(null);
    try {
      await api.cancelWorkflow(runId);
    } catch (err) {
      setStopError(err instanceof Error ? err.message : String(err));
      setStopping(false);
    }
  };

  const loadRun = useCallback(async () => {
    if (runId === null) return;
    try {
      setRun(await api.workflowRun(runId));
    } catch {
      // A run row that cannot be read is not worth blocking the panel over;
      // the live events and diff still work.
    }
  }, [runId]);

  const loadDiff = useCallback(async () => {
    if (runId === null) return;
    setDiffLoading(true);
    setDiffError(null);
    try {
      setDiff(await api.workflowRunDiff(runId));
    } catch (err) {
      setDiffError(err instanceof Error ? err.message : String(err));
    } finally {
      setDiffLoading(false);
    }
  }, [runId]);

  // Agent output arrives as raw stream-json, one JSON object per line, which is
  // exactly what the session renderer already knows how to display.
  //
  // The recorded transcript is read first and the listeners attached after, so
  // a panel opened part-way through a run shows what it already missed without
  // replaying lines the listeners are about to deliver again.
  useEffect(() => {
    if (runId === null) return;
    let disposed = false;
    const unlisteners: (() => void)[] = [];

    const parse = (line: string): ClaudeStreamMessage | null => {
      try {
        return JSON.parse(line) as ClaudeStreamMessage;
      } catch {
        // Non-JSON lines are diagnostics from the CLI, not messages to render.
        return null;
      }
    };

    (async () => {
      const recorded = await api.workflowRunOutput(runId);
      if (disposed) return;
      setMessages(recorded.map(parse).filter((m): m is ClaudeStreamMessage => m !== null));

      const track = (p: Promise<() => void>) => {
        p.then((fn) => (disposed ? fn() : unlisteners.push(fn)));
      };

      track(
        listen<string>(`workflow-output:${runId}`, (e) => {
          if (disposed) return;
          const message = parse(e.payload);
          if (message) setMessages((prev) => [...prev, message]);
        })
      );

      track(
        listen<WorkflowEvent>(`workflow-progress:${runId}`, (e) => {
          if (disposed) return;
          setEvents((prev) => [...prev, e.payload]);
          // A phase change is the cheapest signal that the diff moved on.
          void loadDiff();
          void loadRun();
        })
      );
    })();

    return () => {
      disposed = true;
      unlisteners.forEach((fn) => fn());
    };
  }, [runId, loadDiff, loadRun]);

  useEffect(() => {
    void loadDiff();
    void loadRun();
  }, [loadDiff, loadRun]);

  // A single agent turn can run for minutes without emitting a phase event, so
  // the diff is polled while the run is live rather than sitting stale.
  useEffect(() => {
    if (!running) return;
    const id = setInterval(() => void loadDiff(), LIVE_DIFF_INTERVAL_MS);
    return () => clearInterval(id);
  }, [running, loadDiff]);

  useEffect(() => {
    outputEnd.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages.length, events.length]);

  const reachedIndex = useMemo(
    () => (phase ? PHASES.findIndex((p) => p.phase === phase) : -1),
    [phase]
  );

  const diffLines = useMemo(() => (diff?.patch ? diff.patch.split("\n") : []), [diff]);

  return (
    <motion.aside
      initial={{ x: 40, opacity: 0 }}
      animate={{ x: 0, opacity: 1 }}
      exit={{ x: 40, opacity: 0 }}
      transition={{ duration: 0.18 }}
      className="flex h-full w-[620px] shrink-0 flex-col border-l border-border bg-background"
    >
      <div className="flex items-start justify-between gap-3 border-b border-border px-4 py-3">
        <div className="min-w-0">
          <h3 className="truncate text-sm font-semibold">{ticket.title}</h3>
          <div className="mt-1 flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
            {ticket.branch && (
              <span className="flex items-center gap-1 font-mono">
                <GitBranch className="h-3 w-3" />
                {ticket.branch}
              </span>
            )}
            {ticket.pr_url && (
              <a
                href={ticket.pr_url}
                target="_blank"
                rel="noreferrer"
                className="flex items-center gap-1 hover:text-foreground"
              >
                <GitPullRequest className="h-3 w-3" />
                PR #{ticket.pr_number}
                <ExternalLink className="h-2.5 w-2.5" />
              </a>
            )}
            {runId !== null && <span>run #{runId}</span>}
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {running && (
            <Button
              variant="outline"
              size="sm"
              disabled={stopping}
              onClick={stopRun}
              title="Stops the agent and commits whatever it finished to the branch"
              className="h-7 gap-1 text-xs"
            >
              {stopping ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <Square className="h-3 w-3" />
              )}
              {stopping ? "Stopping…" : "Stop run"}
            </Button>
          )}
          <Button variant="ghost" size="icon" onClick={onClose} title="Close preview">
            <X className="h-4 w-4" />
          </Button>
        </div>
      </div>

      {runId === null ? (
        <div className="flex flex-1 items-center justify-center px-6 text-center text-sm text-muted-foreground">
          This ticket has no run yet. Approve it, then hit “Run agent” to watch the
          work happen here.
        </div>
      ) : (
        <>
          <div className="flex flex-wrap items-center gap-1 border-b border-border px-4 py-2.5">
            {PHASES.map((p, i) => {
              const done = reachedIndex > i || phase === "done";
              const current = reachedIndex === i && !failed;
              return (
                <React.Fragment key={p.phase}>
                  {i > 0 && <span className="text-muted-foreground/40">›</span>}
                  <span
                    className={cn(
                      "flex items-center gap-1 rounded px-1.5 py-0.5 text-xs",
                      done && "text-emerald-400",
                      current && "bg-violet-500/15 font-semibold text-violet-400",
                      !done && !current && "text-muted-foreground/60"
                    )}
                  >
                    {done && <Check className="h-3 w-3" />}
                    {current && running && <Loader2 className="h-3 w-3 animate-spin" />}
                    {p.label}
                  </span>
                </React.Fragment>
              );
            })}
            {waiting && (
              <span className="flex items-center gap-1 rounded bg-violet-500/15 px-1.5 py-0.5 text-xs font-semibold text-violet-400">
                <HelpCircle className="h-3 w-3" />
                Waiting on you
              </span>
            )}
            {cancelled && (
              <span className="flex items-center gap-1 rounded bg-amber-500/15 px-1.5 py-0.5 text-xs font-semibold text-amber-400">
                <Square className="h-3 w-3" />
                Stopped
              </span>
            )}
            {failed && (
              <span className="flex items-center gap-1 rounded bg-red-500/15 px-1.5 py-0.5 text-xs font-semibold text-red-400">
                <AlertTriangle className="h-3 w-3" />
                Failed
              </span>
            )}
          </div>

          {stopError && (
            <div className="border-b border-border px-4 py-2 text-xs text-red-400">
              {stopError}
            </div>
          )}

          {statusLine && (
            <div
              className={cn(
                "border-b border-border px-4 py-2 text-xs",
                failed && "text-red-400",
                waiting && "text-violet-300",
                !failed && !waiting && "text-muted-foreground"
              )}
            >
              {statusLine}
              {latest && latest.max_iterations > 1 && latest.iteration > 0 && (
                <span className="ml-2 opacity-70">
                  (attempt {latest.iteration}/{latest.max_iterations})
                </span>
              )}
            </div>
          )}

          <div className="flex items-center gap-1 border-b border-border px-3 py-2">
            <button
              onClick={() => setTab("output")}
              className={cn(
                "flex items-center gap-1.5 rounded-md px-2.5 py-1 text-xs font-medium transition-colors",
                tab === "output"
                  ? "bg-muted text-foreground"
                  : "text-muted-foreground hover:text-foreground"
              )}
            >
              <Terminal className="h-3.5 w-3.5" />
              Output
            </button>
            <button
              onClick={() => setTab("diff")}
              className={cn(
                "flex items-center gap-1.5 rounded-md px-2.5 py-1 text-xs font-medium transition-colors",
                tab === "diff"
                  ? "bg-muted text-foreground"
                  : "text-muted-foreground hover:text-foreground"
              )}
            >
              <FileDiff className="h-3.5 w-3.5" />
              Diff
            </button>
            <div className="flex-1" />
            <Button
              variant="ghost"
              size="icon"
              onClick={() => void loadDiff()}
              title="Refresh diff"
            >
              <RefreshCw className={cn("h-3.5 w-3.5", diffLoading && "animate-spin")} />
            </Button>
          </div>

          <div className="flex-1 overflow-y-auto">
            {tab === "output" ? (
              <div className="space-y-2 p-3">
                {messages.length === 0 && (
                  <p className="px-1 py-6 text-center text-sm text-muted-foreground">
                    {running
                      ? "Waiting for the agent's first message…"
                      : "This run has no recorded output. Runs from before output was kept show nothing here."}
                  </p>
                )}
                {messages.map((m, i) => (
                  <StreamMessage key={i} message={m} streamMessages={messages} />
                ))}
                <div ref={outputEnd} />
              </div>
            ) : (
              <div className="p-3">
                {diffError && (
                  <div className="rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-sm text-red-400">
                    {diffError}
                  </div>
                )}

                {!diffError && diff && diff.stat && (
                  <pre className="mb-3 overflow-x-auto rounded-lg border border-border/60 bg-muted/30 p-3 font-mono text-xs text-muted-foreground">
                    {diff.stat}
                  </pre>
                )}

                {!diffError && diffLines.length > 0 && (
                  <pre className="overflow-x-auto rounded-lg border border-border/60 bg-muted/20 p-3 font-mono text-xs leading-relaxed">
                    {diffLines.map((line, i) => (
                      <div key={i} className={diffLineClass(line)}>
                        {line || " "}
                      </div>
                    ))}
                  </pre>
                )}

                {!diffError && !diffLoading && diffLines.length === 0 && (
                  <p className="px-1 py-6 text-center text-sm text-muted-foreground">
                    Nothing changed yet.
                  </p>
                )}

                {diff?.truncated && (
                  <p className="mt-2 text-xs text-muted-foreground">
                    Diff was too large to show in full — open the branch to see the rest.
                  </p>
                )}
              </div>
            )}
          </div>
        </>
      )}
    </motion.aside>
  );
};

export default RunPreviewPanel;
