// Pick a teammate or a channel to start talking in.
//
// Controlled by whoever opens it — a `trigger` render prop when it has one of
// its own (the compose affordance that used to sit on `ChannelRail`'s caption
// row), or bare `open`/`onOpenChange` when the trigger lives elsewhere. The
// sidebar's title row (`sidebar-title-row.tsx`) is the latter: its pencil
// button is a sibling of `RoomView`, not a parent, so it cannot render a
// `DialogTrigger` around this dialog itself and instead flips `open` from
// `app-shell.tsx`, which is also where this dialog is mounted — once, reused
// by every opener, per the one-instance rule the rest of this console's
// dialogs already follow (`AddMemberDialog`).

import { useState, type ReactElement } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import type { Channel } from "./model";

interface Props {
  directMessages: Channel[];
  onSelect: (id: string) => void;
  /** Absent when the caller opens it from elsewhere (a menu item) via `open`. */
  trigger?: ReactElement;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** Defaults to the agent-DM wording; a channel picker can reuse this dialog. */
  title?: string;
  description?: string;
}

/** Pick any teammate (or channel, with the right props) to open a composer. */
export function NewMessageDialog({
  directMessages,
  onSelect,
  trigger,
  open: openProp,
  onOpenChange,
  title = "New message",
  description = "Choose an agent to start a direct message.",
}: Props) {
  const [openLocal, setOpenLocal] = useState(false);
  const open = openProp ?? openLocal;
  const setOpen = onOpenChange ?? setOpenLocal;

  function select(id: string) {
    onSelect(id);
    setOpen(false);
  }

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      {trigger && <DialogTrigger render={trigger} />}
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{description}</DialogDescription>
        </DialogHeader>
        <div className="flex max-h-80 flex-col gap-1 overflow-y-auto">
          {directMessages.map((channel) => (
            <Button
              key={channel.id}
              variant="ghost"
              className="h-auto justify-start px-3 py-2 text-left"
              onClick={() => select(channel.id)}
            >
              <span className="min-w-0">
                <span className="block truncate text-sm font-medium">{channel.name}</span>
                {channel.purpose && (
                  <span className="block truncate text-xs font-normal text-muted-foreground">
                    {channel.purpose}
                  </span>
                )}
              </span>
            </Button>
          ))}
        </div>
      </DialogContent>
    </Dialog>
  );
}
