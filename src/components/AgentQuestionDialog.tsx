import React, { useState } from "react";
import { Loader2, HelpCircle } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import { api, type AgentQuestion } from "@/lib/api";

interface AgentQuestionDialogProps {
  question: AgentQuestion;
  /** The ticket the waiting run belongs to, when it is known. */
  ticketTitle?: string;
  onAnswered: () => void;
  /**
   * Set aside without answering. The run stays waiting and the board keeps
   * offering it, because answering may well need a look at the diff first.
   */
  onDismiss: () => void;
}

/** Asks the person the one thing the agent could not decide for itself. */
export const AgentQuestionDialog: React.FC<AgentQuestionDialogProps> = ({
  question,
  ticketTitle,
  onAnswered,
  onDismiss,
}) => {
  const [answer, setAnswer] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const send = async (text: string) => {
    if (!text.trim()) {
      setError("Say something the agent can act on.");
      return;
    }
    setSending(true);
    setError(null);
    try {
      await api.answerWorkflowQuestion(question.run_id, text.trim());
      onAnswered();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setSending(false);
    }
  };

  return (
    <Dialog open onOpenChange={(o) => !o && onDismiss()}>
      <DialogContent className="sm:max-w-[560px]">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <HelpCircle className="h-4 w-4 text-violet-400" />
            The agent needs a decision
          </DialogTitle>
          <DialogDescription>
            {ticketTitle ? (
              <>
                Run #{question.run_id} on “{ticketTitle}” is waiting. It carries on
                as soon as you answer.
              </>
            ) : (
              <>Run #{question.run_id} is waiting. It carries on as soon as you answer.</>
            )}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-2">
          <p className="rounded-lg border border-border/60 bg-muted/30 px-3 py-2.5 text-sm">
            {question.question}
          </p>

          {question.options.length > 0 && (
            <div className="flex flex-wrap gap-2">
              {question.options.map((option) => (
                <Button
                  key={option}
                  variant="outline"
                  size="sm"
                  disabled={sending}
                  onClick={() => send(option)}
                  className={cn("gap-1.5", sending && "opacity-60")}
                >
                  {option}
                </Button>
              ))}
            </div>
          )}

          <Textarea
            value={answer}
            onChange={(e) => setAnswer(e.target.value)}
            placeholder={
              question.options.length > 0
                ? "…or answer in your own words"
                : "Your answer"
            }
            rows={3}
            autoFocus={question.options.length === 0}
            onKeyDown={(e) => {
              // Enter sends; a newline needs the modifier, as in the prompt box.
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                void send(answer);
              }
            }}
          />

          {error && <p className="text-sm text-red-400">{error}</p>}
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={onDismiss} disabled={sending}>
            Not now
          </Button>
          <Button onClick={() => send(answer)} disabled={sending}>
            {sending && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            Send answer
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default AgentQuestionDialog;
