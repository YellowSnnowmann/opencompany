import { ApiError } from "@/api/types";

/** What a failed `/auth/me` read resolves to for a caller that retries. */
export type AdminCheckOutcome = { settled: true; isAdmin: boolean } | { settled: false };

/**
 * Classifies a `fetchMe` failure for a reader that keeps the answer around for
 * the life of a mount.
 *
 * {@link useCanManage} catches every failure as `false`, which is right for a
 * control that only has to fail closed. A reader that never asks again needs
 * the distinction: a definitive `401` — no session on this host at all — is a
 * real answer about who this user is and settles. Anything else (another
 * status, or a raw `fetch` throw that never reached the host) is not an answer,
 * and `settled: false` tells the caller to retry rather than pin a guess for
 * the rest of the mount.
 */
export function resolveAdminCheckError(error: unknown): AdminCheckOutcome {
  if (error instanceof ApiError && error.status === 401) {
    return { settled: true, isAdmin: false };
  }
  return { settled: false };
}
