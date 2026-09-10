import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { motion, AnimatePresence } from "framer-motion";
import {
  CircleDot,
  GitPullRequest,
  ExternalLink,
  Plus,
  Sparkles,
  Loader2,
  RefreshCw,
  Trash2,
  FolderCode,
  Pencil,
  GitBranch,
  AlertTriangle,
  Archive,
  HelpCircle,
  ListChecks,
  EyeOff,
  GitMerge,
  Upload,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import {
  api,
  type AgentQuestion,
  type Agent,
  type ModuleEvent,
  type BoardProject,
  type Preflight,
  type Ticket,
  type TicketPriority,
  type TicketStatus,
} from "@/lib/api";
import { listen } from "@/lib/events";
import type { WorkflowEvent } from "@/lib/api";
import { NewTicketDialog } from "./NewTicketDialog";
import { ImportTicketsDialog } from "./ImportTicketsDialog";
import { TestReportExport } from "./TestReportExport";
import { AgentQuestionDialog } from "./AgentQuestionDialog";
import { RunPreviewPanel } from "./RunPreviewPanel";

/** Column definitions, in the order they appear on the board. */
const COLUMNS: { status: TicketStatus; label: string; dot: string }[] = [
  { status: "pending", label: "Pending", dot: "bg-amber-400" },
  { status: "approved", label: "Approved", dot: "bg-blue-400" },
  { status: "in_progress", label: "In Progress", dot: "bg-violet-400" },
  { status: "completed", label: "Completed", dot: "bg-emerald-400" },
  { status: "cancelled", label: "Cancelled", dot: "bg-muted-foreground/50" },
];

/**
 * Runs a queue will start for one ticket before moving on. Mirrors
 * `MAX_TICKET_ATTEMPTS` in `commands/tickets.rs`, which is what actually
 * enforces it; this is only so a card can say when a ticket has been passed
 * over rather than leaving it looking untouched.
 */
const MAX_TICKET_ATTEMPTS = 2;

const PRIORITY_STYLES: Record<TicketPriority, string> = {
  critical: "bg-red-500/15 text-red-400 border-red-500/30",
  high: "bg-orange-500/15 text-orange-400 border-orange-500/30",
  medium: "bg-amber-500/15 text-amber-400 border-amber-500/30",
  low: "bg-slate-500/15 text-slate-400 border-slate-500/30",
};

const PR_STATE_STYLES: Record<string, string> = {
  merged: "bg-violet-500/15 text-violet-400 border-violet-500/30",
  open: "bg-emerald-500/15 text-emerald-400 border-emerald-500/30",
  draft: "bg-slate-500/15 text-slate-400 border-slate-500/30",
  closed: "bg-red-500/15 text-red-400 border-red-500/30",
};

/** Coarse "x ago" phrasing; the board never needs second-level precision. */
export function formatRelativeTime(iso: string): string {
  const then = new Date(iso.endsWith("Z") || iso.includes("+") ? iso : `${iso}Z`).getTime();
  if (Number.isNaN(then)) return "";

  const seconds = Math.max(0, Math.floor((Date.now() - then) / 1000));
  if (seconds < 60) return "just now";

  const units: [number, string][] = [
    [60, "minute"],
    [3600, "hour"],
    [86400, "day"],
    [2592000, "month"],
    [31536000, "year"],
  ];

  let chosen = units[0];
  for (const unit of units) {
    if (seconds >= unit[0]) chosen = unit;
  }
  const value = Math.floor(seconds / chosen[0]);
  return `${value} ${chosen[1]}${value === 1 ? "" : "s"} ago`;
}

interface TicketCardProps {
  ticket: Ticket;
  agents: Agent[];
  busy: boolean;
  selected: boolean;
  onSelect: (t: Ticket) => void;
  /** Why a run cannot start right now, if it cannot. */
  runBlocker?: string | null;
  onApprove: (t: Ticket) => void;
  onStart: (t: Ticket) => void;
  onDelete: (t: Ticket) => void;
}

const TicketCard: React.FC<TicketCardProps> = ({
  ticket,
  agents,
  busy,
  selected,
  onSelect,
  runBlocker,
  onApprove,
  onStart,
  onDelete,
}) => {
  const prStyle = PR_STATE_STYLES[ticket.pr_state ?? ""] ?? PR_STATE_STYLES.draft;
  const agent = agents.find((a) => a.id === ticket.agent_id) ?? null;
  const exhausted =
    ticket.attempts >= MAX_TICKET_ATTEMPTS &&
    (ticket.status === "pending" || ticket.status === "approved");

  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, scale: 0.97 }}
      transition={{ duration: 0.15 }}
      onClick={() => onSelect(ticket)}
      className={cn(
        "group cursor-pointer rounded-lg border bg-card/80 p-3 transition-colors",
        selected ? "border-primary/70 ring-1 ring-primary/40" : "border-border/60 hover:border-border"
      )}
    >
      <div className="flex items-start justify-between gap-2">
        <h4 className="text-sm font-semibold leading-snug">{ticket.title}</h4>
        <span
          className={cn(
            "shrink-0 rounded border px-1.5 py-0.5 text-[10px] font-bold uppercase tracking-wide",
            PRIORITY_STYLES[ticket.priority]
          )}
        >
          {ticket.priority}
        </span>
      </div>

      <div className="mt-2 flex items-center justify-between gap-2 text-xs text-muted-foreground">
        <span className="truncate">{ticket.epic ?? "—"}</span>
        {agent ? (
          <span
            className="shrink-0 truncate"
            title="Chosen from what this ticket asks for"
          >
            {agent.icon} {agent.name}
          </span>
        ) : (
          <span className="shrink-0 text-amber-400" title="No agents are installed">
            no agent
          </span>
        )}
      </div>

      {(ticket.issue_url || ticket.pr_url) && (
        <div className="mt-2 flex flex-wrap items-center gap-3 text-xs">
          {ticket.issue_url && (
            <a
              href={ticket.issue_url}
              target="_blank"
              rel="noreferrer"
              className="flex items-center gap-1 text-muted-foreground hover:text-foreground"
            >
              <CircleDot className="h-3 w-3" />
              Issue #{ticket.issue_number}
              <ExternalLink className="h-2.5 w-2.5" />
            </a>
          )}
          {ticket.pr_url && (
            <a
              href={ticket.pr_url}
              target="_blank"
              rel="noreferrer"
              className="flex items-center gap-1 text-muted-foreground hover:text-foreground"
            >
              <GitPullRequest className="h-3 w-3" />
              PR #{ticket.pr_number}
              <span
                className={cn(
                  "rounded border px-1 py-px text-[9px] font-bold uppercase",
                  prStyle
                )}
              >
                {ticket.pr_state}
              </span>
              <ExternalLink className="h-2.5 w-2.5" />
            </a>
          )}
        </div>
      )}

      {exhausted && (
        <p className="mt-2 text-xs text-amber-400">
          Failed {ticket.attempts} times — the queue has moved past this one. Run
          it by hand to try again.
        </p>
      )}

      <div className="mt-3 flex items-center justify-between gap-2">
        <span className="text-xs text-muted-foreground">
          {formatRelativeTime(ticket.created_at)}
        </span>

        <div className="flex items-center gap-1">
          <button
            onClick={(e) => {
              e.stopPropagation();
              onDelete(ticket);
            }}
            title="Delete ticket"
            className="rounded p-1 text-muted-foreground opacity-0 transition-opacity hover:text-red-400 group-hover:opacity-100"
          >
            <Trash2 className="h-3.5 w-3.5" />
          </button>

          {ticket.status === "pending" && (
            <Button
              size="sm"
              disabled={busy}
              onClick={(e) => {
                e.stopPropagation();
                onApprove(ticket);
              }}
              className="h-7 gap-1 px-2 text-xs"
            >
              <Sparkles className="h-3 w-3" />
              Approve
            </Button>
          )}

          {ticket.status === "approved" && (
            <Button
              size="sm"
              variant="secondary"
              disabled={busy || !agent || !!runBlocker}
              title={agent ? (runBlocker ?? undefined) : "Assign an agent first"}
              onClick={(e) => {
                e.stopPropagation();
                onStart(ticket);
              }}
              className="h-7 gap-1 px-2 text-xs"
            >
              {busy ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <Sparkles className="h-3 w-3" />
              )}
              Run agent
            </Button>
          )}

          {ticket.status === "in_progress" && (
            <span
              className="flex items-center gap-1 text-xs text-violet-400"
              title="Open the ticket to watch what the agent is doing"
            >
              <Loader2 className="h-3 w-3 animate-spin" />
              {/* A run takes minutes, and a bare "Running" gives no way to tell
                  work in progress from something that died. */}
              Running {formatRelativeTime(ticket.updated_at).replace(" ago", "")}
            </span>
          )}
        </div>
      </div>
    </motion.div>
  );
};

/** An on/off board setting, shown as a chip so its state is readable at rest. */
const SettingChip: React.FC<{
  icon: React.ReactNode;
  label: string;
  on: boolean;
  disabled?: boolean;
  title?: string;
  onClick: () => void;
}> = ({ icon, label, on, disabled, title, onClick }) => (
  <button
    onClick={onClick}
    disabled={disabled}
    title={title}
    className={cn(
      "flex items-center gap-1.5 rounded border px-2 py-1 text-xs transition-colors",
      disabled && "cursor-not-allowed opacity-40",
      on
        ? "border-emerald-500/40 bg-emerald-500/10 text-emerald-400"
        : "border-border/60 text-muted-foreground hover:text-foreground"
    )}
  >
    {icon}
    {label}
  </button>
);

export interface TaskBoardProps {
  /** Repository the board tracks work for. One board per project path. */
  projectPath: string;
  /** Display name, defaulting to the project directory. */
  projectName?: string;
  className?: string;
}

/**
 * Ticket board for one project, tracking implementation work from approval
 * through to the pull request an agent opened for it.
 *
 * The board belongs to the project rather than being set up separately: it is
 * created the first time the project's board is opened.
 */
export const TaskBoard: React.FC<TaskBoardProps> = ({
  projectPath,
  projectName,
  className,
}) => {
  const [board, setBoard] = useState<BoardProject | null>(null);
  const [tickets, setTickets] = useState<Ticket[]>([]);
  // Read inside a listener that must not be torn down and rebuilt on every
  // ticket change, or a question could arrive while nothing is listening.
  const ticketsRef = useRef<Ticket[]>([]);
  const [agents, setAgents] = useState<Agent[]>([]);
  const [loading, setLoading] = useState(true);
  const [busyTicketId, setBusyTicketId] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [preflight, setPreflight] = useState<Preflight | null>(null);
  const [initialising, setInitialising] = useState(false);
  const [showNewTicket, setShowNewTicket] = useState(false);
  const [showImport, setShowImport] = useState(false);
  const [editingTestCommand, setEditingTestCommand] = useState(false);
  const [testCommandDraft, setTestCommandDraft] = useState("");
  const [previewTicketId, setPreviewTicketId] = useState<number | null>(null);
  const [moduleStatus, setModuleStatus] = useState<ModuleEvent | null>(null);
  const [question, setQuestion] = useState<AgentQuestion | null>(null);
  const [questionOpen, setQuestionOpen] = useState(false);

  // Resolved from the live list rather than held as its own copy, so the panel
  // follows the ticket as a run moves it between columns.
  const previewTicket = useMemo(
    () => tickets.find((t) => t.id === previewTicketId) ?? null,
    [tickets, previewTicketId]
  );

  const boardId = board?.id ?? null;

  const skippedCount = useMemo(
    () =>
      tickets.filter(
        (t) =>
          t.attempts >= MAX_TICKET_ATTEMPTS &&
          (t.status === "pending" || t.status === "approved")
      ).length,
    [tickets]
  );

  const questionTicket = useMemo(
    () => tickets.find((t) => t.workflow_run_id === question?.run_id) ?? null,
    [tickets, question]
  );

  // The runner refuses a project that is not a repository or has a dirty tree.
  // Both are worth knowing before starting a run rather than after one fails.
  const loadPreflight = useCallback(async () => {
    try {
      setPreflight(await api.workflowPreflight(projectPath));
    } catch {
      setPreflight(null);
    }
  }, [projectPath]);

  const loadTickets = useCallback(async (projectId: number | null) => {
    if (projectId === null) {
      setTickets([]);
      return;
    }
    setTickets(await api.listTickets(projectId));
  }, []);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      setLoading(true);
      setError(null);
      try {
        const [resolved] = await Promise.all([
          api.boardForProject(projectPath, projectName),
          api.listAgents().then((list) => {
            if (!cancelled) setAgents(list);
          }),
          loadPreflight(),
        ]);
        if (cancelled) return;
        setBoard(resolved);
        await loadTickets(resolved.id);
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [projectPath, projectName, loadTickets, loadPreflight]);

  // A workflow run drives its ticket's column from the backend, so refresh the
  // board whenever a run reports progress rather than polling for it.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    listen<WorkflowEvent>("workflow-progress", (e) => {
      if (disposed) return;
      void loadTickets(boardId);
      void loadPreflight();
      // A run that has moved on is no longer waiting on anything.
      if (e.payload.phase !== "waiting") {
        setQuestion((current) =>
          current && current.run_id === e.payload.run_id ? null : current
        );
      }
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [boardId, loadTickets, loadPreflight]);

  // A module reports on itself: its tickets run side by side, so no single run
  // can say how the group is getting on.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    listen<ModuleEvent>("module-progress", (e) => {
      if (disposed || e.payload.project_path !== projectPath) return;
      setModuleStatus(e.payload);
      void loadTickets(boardId);
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [projectPath, boardId, loadTickets]);

  // Questions arrive on the open channel so one can be raised without the run's
  // panel being open. Only this board's runs are shown: another project's
  // question is not this board's business.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    listen<AgentQuestion>("workflow-question", (e) => {
      if (disposed) return;
      const mine = ticketsRef.current.some(
        (t) => t.workflow_run_id === e.payload.run_id
      );
      if (!mine) return;
      setQuestion(e.payload);
      setQuestionOpen(true);
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const initRepository = async () => {
    setInitialising(true);
    setError(null);
    try {
      setPreflight(await api.initProjectRepository(projectPath));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setInitialising(false);
    }
  };

  const retrySkipped = async () => {
    if (!board) return;
    setError(null);
    try {
      await api.resetSkippedTickets(board.id);
      await loadTickets(board.id);
      await kickQueue();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const ignoreBuildOutput = async () => {
    setInitialising(true);
    setError(null);
    try {
      setPreflight(await api.ignoreBuildOutput(projectPath));
      await kickQueue();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setInitialising(false);
    }
  };

  const stashChanges = async () => {
    setInitialising(true);
    setError(null);
    try {
      setPreflight(await api.stashProjectChanges(projectPath));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setInitialising(false);
    }
  };

  /**
   * Work appearing is not the only thing that starts a queue - switching it on
   * with tickets already waiting should too.
   */
  const kickQueue = useCallback(async () => {
    await api.resumeQueue(projectPath);
    await loadTickets(boardId);
  }, [projectPath, boardId, loadTickets]);

  const setPublishing = async (change: {
    autoPush?: boolean;
    openPr?: boolean;
    autoApprove?: boolean;
    mergeToMain?: boolean;
  }) => {
    if (!board) return;
    try {
      const updated = await api.updateBoardProject({ id: board.id, ...change });
      setBoard(updated);
      if (updated.auto_approve) await kickQueue();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const saveTestCommand = async () => {
    if (!board) return;
    setEditingTestCommand(false);
    if ((board.test_command ?? "") === testCommandDraft.trim()) return;
    try {
      setBoard(
        await api.updateBoardProject({ id: board.id, testCommand: testCommandDraft.trim() })
      );
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    ticketsRef.current = tickets;
  }, [tickets]);

  const byStatus = useMemo(() => {
    const grouped: Record<TicketStatus, Ticket[]> = {
      pending: [],
      approved: [],
      in_progress: [],
      completed: [],
      cancelled: [],
    };
    for (const t of tickets) grouped[t.status]?.push(t);
    return grouped;
  }, [tickets]);

  const handleApprove = async (ticket: Ticket) => {
    setBusyTicketId(ticket.id);
    setError(null);
    try {
      const updated = await api.setTicketStatus(ticket.id, "approved");
      setTickets((prev) => prev.map((t) => (t.id === updated.id ? updated : t)));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyTicketId(null);
    }
  };

  const handleStart = async (ticket: Ticket) => {
    if (!board) return;

    // Unreachable while the button is disabled; the banner is already showing
    // the reason, so repeating it as an error would just say it twice.
    if (preflight?.blocker) return;

    // An agent is chosen when a ticket is created, so this only happens when
    // none were installed at the time - the backend would otherwise fail with
    // an opaque "agent 0 not found".
    if (ticket.agent_id === null) {
      setError(
        `“${ticket.title}” has no agent. Import some under CC Agents, then recreate it.`
      );
      return;
    }

    setBusyTicketId(ticket.id);
    setError(null);
    try {
      await api.startWorkflow({
        agentId: ticket.agent_id,
        projectPath: board.repo_path,
        task: ticket.description?.trim() ? ticket.description : ticket.title,
        testCommand: board.test_command ?? undefined,
        ticketId: ticket.id,
        autoPush: board.auto_push,
        openPr: board.open_pr,
        mergeToMain: board.merge_to_main,
      });
      // Watching the run is the point, so the preview opens with it.
      setPreviewTicketId(ticket.id);
      await loadTickets(boardId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyTicketId(null);
    }
  };

  const handleDelete = async (ticket: Ticket) => {
    try {
      await api.deleteTicket(ticket.id);
      setTickets((prev) => prev.filter((t) => t.id !== ticket.id));
      setPreviewTicketId((current) => (current === ticket.id ? null : current));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  if (loading) {
    return (
      <div className="flex h-full items-center justify-center">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    );
  }

  return (
    <div className={cn("flex h-full flex-col", className)}>
      <div className="flex items-start justify-between gap-4 px-6 pt-6">
        <div className="min-w-0">
          <h1 className="truncate text-2xl font-bold tracking-tight">
            {board?.name ?? projectName ?? "Tasks"}
          </h1>
          <p className="flex items-center gap-1.5 truncate text-sm text-muted-foreground">
            <FolderCode className="h-3.5 w-3.5 shrink-0" />
            <span className="truncate font-mono text-xs">{projectPath}</span>
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              void loadTickets(boardId);
              void loadPreflight();
            }}
            title="Refresh"
          >
            <RefreshCw className="h-4 w-4" />
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={!board}
            onClick={() => setShowImport(true)}
            title="Create many tickets from a spreadsheet"
            className="gap-1"
          >
            <Upload className="h-4 w-4" />
            Import
          </Button>
          <TestReportExport
            projectPath={projectPath}
            size="sm"
            // A Tester run finishing is what makes a report appear, and that
            // shows up here as the tickets reloading. The array's identity
            // changes on every load, so a status change counts too - a count
            // would not.
            refreshToken={tickets}
          />
          <Button
            size="sm"
            disabled={!board}
            onClick={() => setShowNewTicket(true)}
            className="gap-1"
          >
            <Plus className="h-4 w-4" />
            Ticket
          </Button>
        </div>
      </div>

      <div className="flex items-center justify-between gap-4 px-6 py-4">
        <div className="flex min-w-0 items-center gap-2 text-sm">
          <span className="shrink-0 text-muted-foreground">Test command</span>
          {editingTestCommand ? (
            <Input
              autoFocus
              value={testCommandDraft}
              onChange={(e) => setTestCommandDraft(e.target.value)}
              onBlur={saveTestCommand}
              onKeyDown={(e) => {
                if (e.key === "Enter") void saveTestCommand();
                if (e.key === "Escape") setEditingTestCommand(false);
              }}
              placeholder="bun test"
              className="h-7 w-[280px] font-mono text-xs"
            />
          ) : (
            <button
              onClick={() => {
                setTestCommandDraft(board?.test_command ?? "");
                setEditingTestCommand(true);
              }}
              disabled={!board}
              className={cn(
                "flex items-center gap-1.5 rounded border px-2 py-1 font-mono text-xs transition-colors",
                board?.test_command
                  ? "border-border/60 text-muted-foreground hover:border-border hover:text-foreground"
                  : "border-amber-500/40 bg-amber-500/10 text-amber-400"
              )}
              title={
                board?.test_command
                  ? "Runs after every agent turn; must exit 0"
                  : "Without one, agents write code but nothing verifies it"
              }
            >
              <Pencil className="h-3 w-3" />
              {board?.test_command ?? "not set"}
            </button>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <SettingChip
            icon={<ListChecks className="h-3 w-3" />}
            label="Auto-continue"
            on={board?.auto_approve ?? false}
            disabled={!board}
            onClick={() => setPublishing({ autoApprove: !board?.auto_approve })}
            title={
              board?.auto_approve
                ? "Works through the plan a module at a time, without being asked"
                : "Off: each ticket waits to be approved and started by hand"
            }
          />
          <SettingChip
            icon={<GitMerge className="h-3 w-3" />}
            label="Merge to main"
            on={board?.merge_to_main ?? false}
            disabled={!board || !board.test_command}
            onClick={() => setPublishing({ mergeToMain: !board?.merge_to_main })}
            title={
              // multi-branch disabled: every ticket now commits straight to the
              // default branch, so this toggle only changes the wording of the
              // module summary. Restore the backend branch-per-ticket logic to
              // give it teeth again.
              !board?.test_command
                ? "Needs a test command: unverified work is not verified"
                : "Tickets commit directly to the default branch (branch-per-ticket is disabled)"
            }
          />
          <SettingChip
            icon={<GitBranch className="h-3 w-3" />}
            label="Push branch"
            on={board?.auto_push ?? false}
            disabled={!board}
            onClick={() => setPublishing({ autoPush: !board?.auto_push })}
            title={
              board?.auto_push
                ? "Pushes the default branch to origin once tests pass"
                : "Off: work stays local on the default branch"
            }
          />
          <SettingChip
            icon={<GitPullRequest className="h-3 w-3" />}
            label="Draft PR"
            on={board?.open_pr ?? false}
            disabled={!board?.auto_push}
            onClick={() => setPublishing({ openPr: !board?.open_pr })}
            title={
              board?.auto_push
                ? "Opens a draft PR for the pushed branch"
                : "Needs pushing turned on"
            }
          />
          <span className="text-sm text-muted-foreground">
            {tickets.length} task{tickets.length === 1 ? "" : "s"}
          </span>
        </div>
      </div>

      {moduleStatus && (
        <div
          className={cn(
            "mx-6 mb-3 flex items-start gap-2 rounded-lg border px-3 py-2 text-sm",
            moduleStatus.ok === false
              ? "border-red-500/30 bg-red-500/10 text-red-400"
              : moduleStatus.ok === true
                ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-400"
                : "border-violet-500/30 bg-violet-500/10 text-violet-300"
          )}
        >
          {moduleStatus.ok === null ? (
            <Loader2 className="mt-0.5 h-4 w-4 shrink-0 animate-spin" />
          ) : (
            <ListChecks className="mt-0.5 h-4 w-4 shrink-0" />
          )}
          <span className="flex-1">
            <span className="font-medium">
              {moduleStatus.module || "Ungrouped"}
            </span>{" "}
            — {moduleStatus.message}
          </span>
        </div>
      )}

      {question && !questionOpen && (
        <button
          onClick={() => setQuestionOpen(true)}
          className="mx-6 mb-3 flex items-start gap-2 rounded-lg border border-violet-500/40 bg-violet-500/10 px-3 py-2 text-left text-sm text-violet-300 transition-colors hover:bg-violet-500/20"
        >
          <HelpCircle className="mt-0.5 h-4 w-4 shrink-0" />
          <span className="flex-1">
            {questionTicket ? `“${questionTicket.title}” is` : "A run is"} waiting on
            you: {question.question}
          </span>
          <span className="shrink-0 font-medium underline">Answer</span>
        </button>
      )}

      {skippedCount > 0 && (
        <div className="mx-6 mb-3 flex items-start gap-3 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-sm text-amber-400">
          <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
          <span className="flex-1">
            {skippedCount} ticket{skippedCount === 1 ? "" : "s"} the queue has
            given up on. If they failed for a reason you have since fixed, put
            them back in the queue.
          </span>
          <Button
            size="sm"
            variant="outline"
            onClick={retrySkipped}
            className="h-7 shrink-0 gap-1 border-amber-500/40 text-xs text-amber-400 hover:bg-amber-500/20"
          >
            <RefreshCw className="h-3 w-3" />
            Try them again
          </Button>
        </div>
      )}

      {preflight?.blocker && (
        <div className="mx-6 mb-3 flex items-start gap-3 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-sm text-amber-400">
          <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
          <span className="flex-1">{preflight.blocker}</span>
          {preflight.ignorable.length > 0 && (
            <Button
              size="sm"
              variant="outline"
              disabled={initialising}
              onClick={ignoreBuildOutput}
              title={`Adds ${preflight.ignorable.join(", ")} to .gitignore and commits it. Only build output — new source files are left alone.`}
              className="h-7 shrink-0 gap-1 border-amber-500/40 text-xs text-amber-400 hover:bg-amber-500/20"
            >
              {initialising ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <EyeOff className="h-3 w-3" />
              )}
              Ignore them
            </Button>
          )}

          {preflight.is_dirty && (
            <Button
              size="sm"
              variant="outline"
              disabled={initialising}
              onClick={stashChanges}
              title="Runs `git stash push --include-untracked`. Restore it later with `git stash pop`."
              className="h-7 shrink-0 gap-1 border-amber-500/40 text-xs text-amber-400 hover:bg-amber-500/20"
            >
              {initialising ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <Archive className="h-3 w-3" />
              )}
              Stash changes
            </Button>
          )}

          {(!preflight.is_git_repo || !preflight.has_commits) && (
            <Button
              size="sm"
              variant="outline"
              disabled={initialising}
              onClick={initRepository}
              title={
                preflight.is_git_repo
                  ? "Commits the directory's current contents so a run has a base to branch from"
                  : "Runs `git init`, then commits the directory's current contents"
              }
              className="h-7 shrink-0 gap-1 border-amber-500/40 text-xs text-amber-400 hover:bg-amber-500/20"
            >
              {initialising ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <GitBranch className="h-3 w-3" />
              )}
              {preflight.is_git_repo ? "Create first commit" : "Initialize repository"}
            </Button>
          )}
        </div>
      )}

      {error && (
        <div className="mx-6 mb-3 rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-sm text-red-400">
          {error}
        </div>
      )}

      <div className="flex min-h-0 flex-1">
        <div className="flex-1 overflow-x-auto px-6 pb-6">
          <div className="flex h-full min-w-max gap-4">
            {COLUMNS.map((col) => {
              const items = byStatus[col.status];
              return (
                <div
                  key={col.status}
                  className="flex h-full w-[300px] flex-col rounded-xl border border-border/50 bg-muted/20"
                >
                  <div className="flex items-center justify-between px-3 py-2.5">
                    <div className="flex items-center gap-2">
                      <span className={cn("h-2 w-2 rounded-full", col.dot)} />
                      <span className="text-sm font-semibold">{col.label}</span>
                    </div>
                    <span className="text-sm text-muted-foreground">{items.length}</span>
                  </div>

                  <div className="flex-1 space-y-2 overflow-y-auto px-2 pb-2">
                    <AnimatePresence mode="popLayout">
                      {items.map((t) => (
                        <TicketCard
                          key={t.id}
                          ticket={t}
                          agents={agents}
                          busy={busyTicketId === t.id}
                          selected={previewTicketId === t.id}
                          runBlocker={preflight?.blocker ?? null}
                          onSelect={(picked) =>
                            setPreviewTicketId((current) =>
                              current === picked.id ? null : picked.id
                            )
                          }
                          onApprove={handleApprove}
                          onStart={handleStart}
                          onDelete={handleDelete}
                        />
                      ))}
                    </AnimatePresence>

                    {items.length === 0 && (
                      <div className="flex h-24 items-center justify-center text-sm text-muted-foreground">
                        No tasks
                      </div>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        </div>

        <AnimatePresence>
          {previewTicket && (
            <RunPreviewPanel
              key={previewTicket.id}
              ticket={previewTicket}
              onClose={() => setPreviewTicketId(null)}
            />
          )}
        </AnimatePresence>
      </div>

      {question && questionOpen && (
        <AgentQuestionDialog
          question={question}
          ticketTitle={questionTicket?.title}
          onDismiss={() => setQuestionOpen(false)}
          onAnswered={() => {
            setQuestion(null);
            setQuestionOpen(false);
          }}
        />
      )}

      {showImport && board && (
        <ImportTicketsDialog
          projectId={board.id}
          onClose={() => setShowImport(false)}
          onImported={async () => {
            setShowImport(false);
            await loadTickets(board.id);
            // A sheet dropped onto an idle board is a queue with nobody to
            // start it, so it gets started here.
            await kickQueue();
          }}
        />
      )}

      {showNewTicket && board && (
        <NewTicketDialog
          projectId={board.id}
          onClose={() => setShowNewTicket(false)}
          onCreated={async (ticket) => {
            setShowNewTicket(false);
            setTickets((prev) => [...prev, ticket]);
            await kickQueue();
          }}
        />
      )}
    </div>
  );
};

export default TaskBoard;
