// The look a new teammate is created with — its face and, for a mascot, its
// display mode, costume and two colors.
//
// Three views create teammates (the roster, the chat pane, the org chart) and
// each used to do its own thing with the face the dialog collected: the roster
// wrote it with a second `PATCH`, the other two dropped it. This is the one place
// that decides what a create request carries and what has to be written after it,
// so the three cannot drift again.
//
// The look rides the create request itself, so the teammate is born wearing it
// (the host validates it against the same closed lists as the `PATCH` route). A
// host that predates the fields ignores them — unknown keys are dropped — and
// answers without echoing them; that is how this notices, and it then writes
// only what was not echoed the old way, with a `PATCH`.

import type { OpenCompanyClient } from "@/api/client";
import type { EditAgentInput, TeamMemberDto } from "@/api/types";

/** The look fields a create request can carry. Absent means "nobody chose". */
export interface NewMemberLook {
  avatar?: string;
  mascotMode?: string;
  mascotCostume?: string;
  mascotSkinColor?: string;
  mascotHandColor?: string;
}

const LOOK_KEYS = ["avatar", "mascotMode", "mascotCostume", "mascotSkinColor", "mascotHandColor"] as const;

/**
 * The look as it goes on the wire: only what was chosen. A blank is dropped, not
 * sent — at creation there is nothing to reset to, so an empty string means "no
 * choice", never a stored empty value.
 */
export function birthLook(fields: NewMemberLook): NewMemberLook {
  const look: NewMemberLook = {};
  for (const key of LOOK_KEYS) {
    const value = fields[key]?.trim();
    if (value) look[key] = value;
  }
  return look;
}

/**
 * What the host did not take from the create request: the requested look fields
 * its response does not echo back. `null` when there is nothing left to write —
 * the common case, on a host that has the fields.
 */
export function unechoedLook(created: TeamMemberDto, fields: NewMemberLook): EditAgentInput | null {
  const wanted = birthLook(fields);
  const missing: EditAgentInput = {};
  let any = false;
  for (const key of LOOK_KEYS) {
    const value = wanted[key];
    if (value !== undefined && created[key] !== value) {
      missing[key] = value;
      any = true;
    }
  }
  return any ? missing : null;
}

/**
 * Writes whatever of the look the create response did not echo, and answers
 * whether everything landed. Best effort by design: a teammate with the wrong
 * face is still a teammate, so a failure is reported (the callers name it in the
 * add outcome), never thrown.
 */
export async function writeUnechoedLook(
  client: OpenCompanyClient,
  company: string | null | undefined,
  created: TeamMemberDto,
  fields: NewMemberLook,
): Promise<boolean> {
  const missing = unechoedLook(created, fields);
  if (!missing) return true;
  try {
    await client.updateAgent(created.id, missing, company);
    return true;
  } catch {
    return false;
  }
}
