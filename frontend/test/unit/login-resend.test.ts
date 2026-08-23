import { describe, expect, it } from "vitest";

import {
  RESEND_INTERVAL_MILLIS,
  resendLabel,
  secondsUntilResend,
} from "@/views/login/resend";

/**
 * The resend clock on Login's "check your email" card (issue #1333).
 *
 * These assertions stand in for a question the console cannot ask the host.
 * `auth/request` answers `202 {sent: true}` whether it mailed a link or
 * silently swallowed the request inside its one-minute window, so a resend
 * button that fires blind reports sends that never happened. The countdown is
 * the console's own copy of that window, and these tests are what keep the copy
 * honest at its two dangerous ends: the moment before it opens, and a clock
 * that has jumped.
 */
describe("secondsUntilResend", () => {
  const sent = 1_700_000_000_000;

  it("matches the host's window the instant a link goes out", () => {
    expect(secondsUntilResend(sent, sent)).toBe(RESEND_INTERVAL_MILLIS / 1000);
  });

  it("counts down in whole seconds", () => {
    expect(secondsUntilResend(sent, sent + 15_000)).toBe(45);
    expect(secondsUntilResend(sent, sent + 59_000)).toBe(1);
  });

  it("rounds up, so the last fraction of a second is never advertised as ready", () => {
    // The host started its window when it minted the code, fractionally before
    // this response landed. Rounding down here would enable the button while
    // the other end is still shut, and the resulting 202 would look like a send.
    expect(secondsUntilResend(sent, sent + 59_001)).toBe(1);
    expect(secondsUntilResend(sent, sent + 59_999)).toBe(1);
  });

  it("opens exactly at the interval, and stays open", () => {
    expect(secondsUntilResend(sent, sent + RESEND_INTERVAL_MILLIS)).toBe(0);
    expect(secondsUntilResend(sent, sent + 10 * RESEND_INTERVAL_MILLIS)).toBe(0);
  });

  it("caps at the interval when the clock moves backwards", () => {
    // A resumed laptop or an NTP step, not a forty-minute wait to render.
    expect(secondsUntilResend(sent, sent - 40 * 60_000)).toBe(60);
  });
});

describe("resendLabel", () => {
  it("carries the wait in the label a screen reader announces", () => {
    // Beside the button it would be read as unrelated text, leaving a disabled
    // control that refuses to say why it is disabled.
    expect(resendLabel(42)).toBe("Resend link in 42s");
  });

  it("drops to the bare action once the window is open", () => {
    expect(resendLabel(0)).toBe("Resend link");
  });
});
