// The left pane: every chat with the company, newest line as the preview.
//
// Not an assistant-ui `ThreadListPrimitive`. That primitive models one
// assistant's *conversations* — new thread, rename, archive — whereas these
// rows are the company's desks and teammates, built by `lib/threads` from the
// host's roster and addressed by desk/agent id. There is nothing to create or
// archive here, and the ids are not assistant-ui's to mint.

import { PenSquare } from "lucide-react";

import { Button } from "@/components/ui/button";
import type { Thread } from "@/lib/threads";
import { cn } from "@/lib/utils";
import { formatTime, previewOf } from "./model";
import { ContactAvatar } from "./parts";

export function ThreadList({
  threads,
  activeId,
  onSelect,
  className,
}: {
  threads: Thread[];
  activeId: string;
  onSelect: (id: string) => void;
  className?: string;
}) {
  return (
    <aside className={cn("min-h-0 w-full shrink-0 flex-col border-r bg-card/40 md:w-80", className)}>
      <div className="flex items-center justify-between px-4 py-3">
        <h2 className="text-sm font-semibold">Chats</h2>
        <Button variant="ghost" size="icon" className="size-8" aria-label="New chat" disabled>
          <PenSquare className="size-4" />
        </Button>
      </div>
      <div className="flex-1 overflow-y-auto px-2 pb-2">
        {threads.map((t) => {
          const last = t.messages[t.messages.length - 1];
          const preview = last ? previewOf(last) : t.blurb;
          return (
            <button
              key={t.id}
              onClick={() => onSelect(t.id)}
              className={cn(
                "flex w-full items-center gap-3 rounded-lg px-2 py-2.5 text-left transition-colors",
                t.id === activeId ? "bg-accent" : "hover:bg-accent/50",
              )}
            >
              <ContactAvatar contact={t.contact} className="size-10" />
              <div className="min-w-0 flex-1">
                <div className="flex items-baseline justify-between gap-2">
                  <span className="truncate text-sm font-medium">{t.contact.name}</span>
                  {last && (
                    <span className="shrink-0 text-2xs text-muted-foreground">
                      {formatTime(last.at)}
                    </span>
                  )}
                </div>
                <p className="truncate text-xs text-muted-foreground">{preview}</p>
              </div>
            </button>
          );
        })}
      </div>
    </aside>
  );
}
