// The Conversation view's own presentation pieces — the bits assistant-ui has
// no opinion about, because they are this product's rather than chat's in
// general: the company/desk avatar, the day separator, the processing-step
// timeline behind a reply, and the two board-card affordances (issues #246 and
// #984).

import { useState } from "react";
import {
  AlertTriangle,
  Brain,
  Building2,
  ChevronDown,
  ChevronRight,
  Loader2,
  SquareKanban,
  Wrench,
  X,
} from "lucide-react";

import type { TurnStep, TurnStepKind } from "@/api/types";
import { Markdown } from "@/components/markdown";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { titleFromMessage, type ChatMessage } from "@/lib/chat";
import type { ThreadContact } from "@/lib/threads";
import { cn } from "@/lib/utils";
import { WorkingIndicator } from "@/views/chat/WorkingIndicator";
import { formatDay, formatElapsed, formatTime, initials, toneClass, type Sender } from "./model";

export function ContactAvatar({ contact, className }: { contact: ThreadContact; className?: string }) {
  if (contact.kind === "company") {
    return (
      <div
        className={cn(
          "flex shrink-0 items-center justify-center rounded-full bg-primary text-primary-foreground",
          className,
        )}
        aria-hidden
      >
        <Building2 className="size-1/2" />
      </div>
    );
  }
  return (
    <div
      className={cn(
        "flex shrink-0 items-center justify-center rounded-full text-xs font-semibold",
        toneClass(contact.tone),
        className,
      )}
      aria-hidden
    >
      {initials(contact.name)}
    </div>
  );
}

export function SenderAvatar({ sender }: { sender: Sender }) {
  return (
    <div className="mt-5">
      <ContactAvatar
        contact={{
          name: sender.name,
          kind: sender.kind === "company" ? "company" : "agent",
          tone: sender.tone,
        }}
        className="size-8"
      />
    </div>
  );
}

export function DaySeparator({ at }: { at: number }) {
  return (
    <div className="my-3 flex items-center gap-3">
      <div className="h-px flex-1 bg-border" />
      <span className="text-2xs font-medium text-muted-foreground">{formatDay(at)}</span>
      <div className="h-px flex-1 bg-border" />
    </div>
  );
}

export function EmptyConversation({ contact }: { contact: ThreadContact }) {
  return (
    <div className="mt-16 flex flex-col items-center gap-3 text-center">
      <ContactAvatar contact={contact} className="size-12" />
      <div className="space-y-1">
        <p className="font-medium">Message {contact.name}</p>
        <p className="max-w-sm text-sm text-muted-foreground">
          Say hello, ask for an update, or hand off a task. Your company handles the rest.
        </p>
      </div>
    </div>
  );
}

export function TypingIndicator({ contact, queued }: { contact: ThreadContact; queued?: boolean }) {
  return (
    <div className="mt-2 flex gap-2.5">
      <ContactAvatar contact={contact} className="mt-0.5 size-8" />
      {/* Same indicator the Chat tab uses (#787), so the two surfaces cannot
          drift into one whimsical and one silent. The bubble shape here is
          this surface's own — only the contents are shared. */}
      <WorkingIndicator
        srLabel="Replying…"
        queued={queued}
        className="rounded-2xl rounded-bl-md border bg-card px-3.5 py-3"
      />
    </div>
  );
}

/* ---- processing-step timeline (Activity-trace) ---- */

/**
 * The scrubbed processing steps behind a company reply, rendered above its
 * bubble. Collapsed by default to a one-line "N steps · M failed" summary; auto
 * expands when any step failed so a silent MCP failure is visible, not buried.
 * Renders nothing when there are no steps (a memory-served / tool-less reply).
 */
export function StepTimeline({ steps }: { steps: TurnStep[] }) {
  const failed = steps.filter((s) => s.status === "error").length;
  const hasError = failed > 0;
  const [open, setOpen] = useState(hasError);

  if (steps.length === 0) return null;

  return (
    <div className="w-full max-w-[85%] sm:max-w-[75%]">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className={cn(
          "flex items-center gap-1 rounded-md px-1.5 py-0.5 text-2xs font-medium transition-colors hover:bg-accent/60",
          hasError ? "text-destructive" : "text-muted-foreground",
        )}
      >
        {open ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />}
        <span>
          {steps.length} step{steps.length === 1 ? "" : "s"}
          {failed > 0 && ` · ${failed} failed`}
        </span>
      </button>
      {open && (
        <ol className="mt-0.5 flex flex-col gap-1 rounded-lg border bg-card/60 px-2.5 py-1.5">
          {steps.map((step, i) => (
            <StepRow key={i} step={step} />
          ))}
        </ol>
      )}
    </div>
  );
}

function StepRow({ step }: { step: TurnStep }) {
  const error = step.status === "error";
  const Icon = stepIcon(step.kind);
  return (
    <li
      className={cn(
        "flex items-center gap-1.5 text-2xs leading-relaxed",
        error ? "text-destructive" : "text-muted-foreground",
      )}
    >
      <Icon className={cn("size-3 shrink-0", step.status === "running" && "animate-pulse")} />
      <span className={cn("font-medium", !error && "text-foreground/80")}>{step.label}</span>
      {step.detail && <span className="min-w-0 truncate">— {step.detail}</span>}
      {typeof step.elapsedMs === "number" && (
        <span className="ml-auto shrink-0 tabular-nums opacity-70">
          {formatElapsed(step.elapsedMs)}
        </span>
      )}
    </li>
  );
}

function stepIcon(kind: TurnStepKind) {
  switch (kind) {
    case "tool_call":
      return Wrench;
    case "thinking":
      return Brain;
    case "note":
      return AlertTriangle;
    default:
      return Wrench;
  }
}

/* ---- bubbles and their board-card affordances ---- */

export function Bubble({
  message,
  mine,
  last,
  onDismissCard,
  dismissingCardId,
}: {
  message: ChatMessage;
  mine: boolean;
  last: boolean;
  onDismissCard: (taskId: string) => void;
  dismissingCardId: string | null;
}) {
  return (
    <div
      className={cn(
        "relative max-w-[85%] rounded-2xl px-3 py-1.5 text-sm leading-relaxed shadow-sm sm:max-w-[75%]",
        mine ? "bg-primary text-primary-foreground" : "border bg-card text-card-foreground",
        last && (mine ? "rounded-br-md" : "rounded-bl-md"),
      )}
    >
      <span
        className={cn(
          "float-right ml-2 translate-y-1 select-none text-3xs",
          mine ? "text-primary-foreground/70" : "text-muted-foreground",
        )}
      >
        {formatTime(message.at)}
      </span>
      {mine ? (
        // User-typed bubbles stay plain text so literal asterisks/underscores a
        // person types aren't reinterpreted as markdown.
        <span className="whitespace-pre-wrap break-words align-bottom">{message.text}</span>
      ) : (
        // Company/agent replies render markdown so **bold**, lists, and links
        // show formatted instead of leaking raw markup. Trim the first/last
        // block margins so a reply stays flush inside the tight bubble padding.
        <Markdown className="[&>:first-child]:mt-0 [&>:last-child]:mb-0">{message.text}</Markdown>
      )}
      {message.taskId && (
        <CardChip
          taskId={message.taskId}
          mine={mine}
          busy={dismissingCardId === message.taskId}
          disabled={dismissingCardId !== null && dismissingCardId !== message.taskId}
          onDismiss={onDismissCard}
        />
      )}
    </div>
  );
}

/**
 * The "this message has a card" chip (issue #246), linking to the card's detail
 * screen.
 *
 * Two provenances, one render. On a company reply it means the turn opened a
 * card by itself — that id is journaled onto the reply, so this chip survives a
 * transcript reload. On your own message it means you pressed "Add to board";
 * that link lives in the session only, because the durable record of it is
 * `originChatId` on the card rather than anything on the operator message.
 *
 * `clear-both` because the bubble floats its timestamp right; without it the
 * chip tucks under the time instead of starting a fresh line.
 */
function CardChip({
  taskId,
  mine,
  busy,
  disabled,
  onDismiss,
}: {
  taskId: string;
  mine: boolean;
  busy: boolean;
  disabled: boolean;
  onDismiss: (taskId: string) => void;
}) {
  const tone = mine
    ? "bg-primary-foreground/15 text-primary-foreground"
    : "bg-accent text-accent-foreground";
  return (
    <span className={cn("mt-1.5 flex w-fit clear-both items-center rounded-full", tone)}>
      <a
        href={`#/tasks/${encodeURIComponent(taskId)}`}
        className="flex items-center gap-1 py-0.5 pl-2 pr-1 text-2xs font-medium transition-opacity hover:opacity-80"
      >
        <SquareKanban className="size-3 shrink-0" />
        {mine ? "Added to the board" : "Card opened"}
      </a>
      <AlertDialog>
        <AlertDialogTrigger
          render={
            <button
              type="button"
              // Always in the DOM and focusable rather than revealed on hover:
              // the chip is the only place this card can be dismissed from
              // chat, and a hover-only control is unreachable by keyboard and
              // on touch. `AddToBoardAction` below can afford hover because
              // its absence costs nothing.
              className="flex items-center rounded-full py-0.5 pl-0.5 pr-1.5 transition-opacity hover:opacity-80 disabled:opacity-50"
              disabled={busy || disabled}
              title="Dismiss this card"
              aria-label="Dismiss this card"
            >
              {busy ? (
                <Loader2 className="size-3 shrink-0 animate-spin" />
              ) : (
                <X className="size-3 shrink-0" />
              )}
            </button>
          }
        />
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Dismiss this card?</AlertDialogTitle>
            <AlertDialogDescription>
              This deletes the card from the board and can’t be undone. The message
              stays in the conversation.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Keep card</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => onDismiss(taskId)}
              className="bg-destructive text-white hover:bg-destructive/90"
            >
              Dismiss card
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </span>
  );
}

/**
 * The per-message "Add to board" affordance (issue #246).
 *
 * On every message in every thread — desk, DM and orchestrator — because it
 * creates through REST rather than through the responder's toolbelt, which only
 * the orchestrator carries.
 *
 * Renders nothing once the message already has a card, so a second press cannot
 * open a duplicate; and nothing for a message with no text to title a card
 * from. Revealed on hover on pointer devices, but always present in the DOM and
 * focusable, so it is reachable by keyboard and on touch.
 */
export function AddToBoardAction({
  message,
  busy,
  disabled,
  onAdd,
}: {
  message: ChatMessage;
  busy: boolean;
  disabled: boolean;
  onAdd: (message: ChatMessage) => void;
}) {
  if (message.taskId || !titleFromMessage(message.text)) return null;
  return (
    <Button
      variant="ghost"
      size="icon"
      className="size-7 shrink-0 text-muted-foreground opacity-0 transition-opacity focus-visible:opacity-100 group-hover/msg:opacity-100"
      onClick={() => onAdd(message)}
      disabled={busy || disabled}
      title="Add to board"
      aria-label="Add to board"
    >
      {busy ? (
        <Loader2 className="size-3.5 animate-spin" />
      ) : (
        <SquareKanban className="size-3.5" />
      )}
    </Button>
  );
}
