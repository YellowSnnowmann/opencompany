// The one place new work enters the board by hand (issue #301, issue #580).
//
// Lifted out of the board screen when that screen was retired (issue #1140).
// The board itself renders inside the Ledgers section as the `tasks` ledger's
// columns, but a ledger's own compose box is `record_entry` — which the host
// refuses for this ledger, because entering a column fires real work. So
// creation keeps its own dialog, posting to `…/tasks` exactly as it always did,
// and Ledgers offers it only for the native board.
//
// Its labels, ids and headings are unchanged on purpose: "Add task",
// "New task", `#new-prompt`. They are what the e2e suite drives, and moving a
// dialog between screens should not rewrite the vocabulary an operator (or a
// test) already knows.

import { useEffect, useState } from "react";
import { Loader2 } from "lucide-react";
import { toast } from "sonner";

import type { OpenCompanyClient } from "@/api/client";
import {
  createTask,
  type CreateTask,
  type Task,
  type TaskDeliverable,
} from "@/api/tasks";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { useBoardColumns } from "@/hooks/use-board-columns";
import { ADD_TASK_COLUMN, labelFor } from "@/lib/board-columns";
import { AssigneeSelect } from "./AssigneeSelect";


/** The card priorities the host and its edit dialog support. */
const PRIORITIES = ["low", "medium", "high"] as const;
type TaskPriority = (typeof PRIORITIES)[number];

/**
 * Splits a prompt into the card's `{title, note}` (issue #301).
 *
 * The prompt box asks "what needs doing?", so what it collects is an *ask*, not
 * a name. It used to cut that ask at its first line and post it as the title —
 * the same rule the chat handler used, and the reason a
 * board of prompt-box cards read as a list of half-sentences. Now the whole
 * prompt goes as the note and the host names the card from it.
 *
 * Returns no title at all rather than a derived one: `title` is optional on the
 * wire precisely so a caller can say "I have no name for this, name it".
 *
 * The invariant that matters is unchanged: the operator's full text always
 * survives on the card. It is simply always in the note now.
 */
export function derivePromptCard(prompt: string): { title?: string; note?: string } {
  const full = prompt.trim();
  return full ? { note: full } : {};
}

/**
 * Builds the `POST …/tasks` body from what the dialog collected, or `null` when
 * there is no title to create from.
 *
 * Extracted for the same reason {@link derivePromptCard} is: the rule worth
 * pinning is what the dialog *omits*. Optional fields are sent only when they
 * differ from the host's own default — `"once"`, `"medium"`, and unassigned are
 * sent as nothing rather than as `"once"`, `"medium"`, and `""` — so a card
 * created without touching the controls posts the body it posted before they
 * existed. That is
 * what keeps `column`'s deliberate absence (issue #301) meaningful: the server's
 * intake default decides where the card lands, and nothing here widens the body
 * far enough to start deciding for it.
 */
export function newTaskBody({
  prompt,
  deliverable,
  priority,
  assignee,
}: {
  prompt: string;
  deliverable: TaskDeliverable;
  priority: TaskPriority;
  /** The wire value: `""` (unassigned), a desk id, or a teammate id. */
  assignee: string;
}): CreateTask | null {
  const { note } = derivePromptCard(prompt);
  if (!note) return null;
  const body: CreateTask = { note };
  if (deliverable === "workflow") body.deliverable = "workflow";
  if (priority !== "medium") body.priority = priority;
  // Issue #1106. Sent verbatim — a desk stays a desk, exactly as
  // `AssigneeSelect` submits it; resolving one to its lead is the host's call
  // and only for the surfaces that are allowed to make it.
  if (assignee) body.assignee = assignee;
  return body;
}

/**
 * The once-vs-workflow options, in review order (issue #580). Shared by the
 * create dialog here and the edit dialog, so the two pickers can never offer a
 * different vocabulary than the wire's {@link TaskDeliverable}.
 */
export const DELIVERABLE_OPTIONS: { value: TaskDeliverable; label: string; hint: string }[] = [
  { value: "once", label: "Do it once", hint: "A one-off result." },
  {
    value: "workflow",
    label: "Build me the automation",
    hint: "A reusable automation you can open, edit and re-run.",
  },
];

/**
 * New work enters the board through one prompt box (issue #301).
 *
 * Title/Note are derived from the prompt. Priority is an explicit choice here
 * and remains editable on the card. `column` is omitted on purpose so the
 * *server's* intake default decides where the card lands, keeping the human
 * drag into In progress the only thing that spends an agent turn.
 *
 * Three fields are collected beyond the prompt.
 *
 * The **deliverable** (issue #580): once versus workflow is a decision about
 * *what kind of thing* the card produces, not a default the host can pick, so
 * the operator states it here. It still lands in To-do like any card — the
 * builder pass fires only on the drag into In progress.
 *
 * The **priority** (issue #1357): a card's priority is its most prominent
 * visual signal, so an operator who knows the urgency states it when creating
 * the card. It defaults to medium, preserving the host's historical default.
 *
 * The **owner** (issue #1106). Assignee was moved out by #301 on the reasoning
 * that the host defaults it and the card edits it. What that missed is what the
 * host's default actually is: an unassigned card is routed by a planning pass
 * that picks from the roster, and when two teammates plausibly fit it picked one
 * with nothing recorded about the other. Offering the picker here is the
 * *pre-empt* for that — an operator who knows the owner states it once and never
 * meets the ambiguity prompt #1106 adds. It defaults to unassigned, so a quick
 * card is exactly as quick as before and today's routing stays the default
 * rather than becoming a choice forced on every card.
 */
export function CreateTaskDialog({
  open,
  onClose,
  onCreated,
  client,
  company,
}: {
  open: boolean;
  onClose: () => void;
  onCreated: (t: Task) => void;
  client: OpenCompanyClient;
  company: string | null;
}) {
  const [prompt, setPrompt] = useState("");
  const [deliverable, setDeliverable] = useState<TaskDeliverable>("once");
  const [priority, setPriority] = useState<TaskPriority>("medium");
  // The wire value, verbatim: `""` (unassigned), a desk id, or a teammate id.
  // `AssigneeSelect` never resolves a desk to its lead, and neither does this.
  const [assignee, setAssignee] = useState("");
  const [busy, setBusy] = useState(false);
  // Above the `!open` return, and it has to be: every hook runs on every
  // render or none do. Reading the columns *below* it made this component call
  // one hook while closed and two while open, which React ends the render with
  // (error #310) — the dialog then never appeared and the console error was the
  // only trace. Found by the e2e suite, not by the type checker.
  const columns = useBoardColumns(client, company);

  useEffect(() => {
    if (open) {
      setPrompt("");
      setDeliverable("once");
      setPriority("medium");
      setAssignee("");
    }
  }, [open]);

  if (!open) return null;

  async function create() {
    const body = newTaskBody({ prompt, deliverable, priority, assignee });
    if (!body) return;
    setBusy(true);
    try {
      const created = await createTask(client, company, body);
      onCreated(created);
      toast.success("Task created.");
    } catch (e) {
      toast.error(e instanceof Error ? e.message : "could not create the task");
    } finally {
      setBusy(false);
    }
  }

  const columnLabel = labelFor(columns, ADD_TASK_COLUMN);

  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      {/* `sm:` — DialogContent's own `sm:max-w-sm` beats an unprefixed width. */}
      <DialogContent className="sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>New task</DialogTitle>
          <DialogDescription>Added to “{columnLabel}”.</DialogDescription>
        </DialogHeader>

        <div className="grid gap-1.5">
          <Label htmlFor="new-prompt">What needs doing?</Label>
          <Textarea
            id="new-prompt"
            autoFocus
            // Textarea is `field-sizing-content`, so `rows` is inert — a
            // min-height is what actually gives the box room.
            className="min-h-32 resize-y"
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            placeholder="Describe the work. The card gets named from it."
          />
        </div>

        <div className="grid gap-1.5">
          <Label htmlFor="new-deliverable">Deliverable</Label>
          <Select
            value={deliverable}
            onValueChange={(v) => setDeliverable((v as TaskDeliverable) ?? "once")}
          >
            <SelectTrigger id="new-deliverable" data-testid="create-deliverable">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {DELIVERABLE_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <p className="text-2xs text-muted-foreground">
            {DELIVERABLE_OPTIONS.find((o) => o.value === deliverable)?.hint}
          </p>
        </div>

        <div className="grid gap-1.5">
          <Label htmlFor="new-assignee">Owner</Label>
          <AssigneeSelect
            id="new-assignee"
            client={client}
            company={company}
            value={assignee}
            onChange={setAssignee}
            disabled={busy}
          />
          <p className="text-2xs text-muted-foreground">
            Leave unassigned and the card is routed for you. If more than one
            agent fits, it waits and asks rather than picking one.
          </p>
        </div>

        <div className="grid gap-1.5">
          <Label htmlFor="new-priority">Priority</Label>
          <Select
            value={priority}
            onValueChange={(v) => setPriority((v as TaskPriority) ?? "medium")}
            disabled={busy}
          >
            <SelectTrigger id="new-priority" data-testid="create-priority">
              <SelectValue className="capitalize" />
            </SelectTrigger>
            <SelectContent>
              {PRIORITIES.map((value) => (
                <SelectItem key={value} value={value} className="capitalize">
                  {value}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button
            variant={prompt.trim() ? "default" : "secondary"}
            onClick={() => void create()}
            disabled={busy || !prompt.trim()}
          >
            {busy && <Loader2 className="mr-1.5 size-4 animate-spin" />}
            Create
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
