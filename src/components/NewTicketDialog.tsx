import React, { useState } from "react";
import { Loader2 } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import { api, type Ticket, type TicketPriority } from "@/lib/api";

const PRIORITIES: TicketPriority[] = ["low", "medium", "high", "critical"];

interface NewTicketDialogProps {
  projectId: number;
  onClose: () => void;
  onCreated: (ticket: Ticket) => void;
}

/**
 * Creates a ticket in the Pending column.
 *
 * The description matters more than it looks: it becomes the task prompt handed
 * to the agent, falling back to the title when left empty.
 */
export const NewTicketDialog: React.FC<NewTicketDialogProps> = ({
  projectId,
  onClose,
  onCreated,
}) => {
  const [title, setTitle] = useState("");
  const [epic, setEpic] = useState("");
  const [description, setDescription] = useState("");
  const [priority, setPriority] = useState<TicketPriority>("medium");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    if (!title.trim()) {
      setError("A title is required.");
      return;
    }

    setSaving(true);
    setError(null);
    try {
      const ticket = await api.createTicket({
        projectId,
        title: title.trim(),
        epic: epic.trim() || undefined,
        description: description.trim() || undefined,
        priority,
      });
      onCreated(ticket);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-[520px]">
        <DialogHeader>
          <DialogTitle>New ticket</DialogTitle>
          <DialogDescription>
            Lands in Pending. Nothing runs until you approve it. The agent is
            chosen from what the ticket asks for.
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-2">
          <div className="space-y-2">
            <Label htmlFor="ticket-title">Title</Label>
            <Input
              id="ticket-title"
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="Build results grid with cards"
              autoFocus
            />
          </div>

          <div className="space-y-2">
            <Label htmlFor="ticket-epic">Epic</Label>
            <Input
              id="ticket-epic"
              value={epic}
              onChange={(e) => setEpic(e.target.value)}
              placeholder="Search & Explore Places"
            />
          </div>

          <div className="space-y-2">
            <Label>Priority</Label>
            <div className="flex gap-2">
              {PRIORITIES.map((p) => (
                <button
                  key={p}
                  type="button"
                  onClick={() => setPriority(p)}
                  className={cn(
                    "flex-1 rounded-md border px-2 py-1.5 text-xs font-semibold uppercase tracking-wide transition-colors",
                    priority === p
                      ? "border-primary bg-primary/15 text-primary"
                      : "border-border text-muted-foreground hover:bg-muted"
                  )}
                >
                  {p}
                </button>
              ))}
            </div>
          </div>

          <div className="space-y-2">
            <Label htmlFor="ticket-description">
              Description{" "}
              <span className="font-normal text-muted-foreground">
                — becomes the agent's task prompt, and chooses which agent
              </span>
            </Label>
            <Textarea
              id="ticket-description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="What needs building, and how you'll know it works."
              rows={4}
            />
          </div>

          {error && <p className="text-sm text-red-400">{error}</p>}
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button onClick={submit} disabled={saving}>
            {saving && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            Create ticket
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default NewTicketDialog;
