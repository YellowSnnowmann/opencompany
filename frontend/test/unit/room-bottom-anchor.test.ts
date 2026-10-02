import { describe, expect, it } from "vitest";

import { BOTTOM_SLACK_PX, isAtBottom, readsAsFollowing } from "@/views/room/bottomAnchor";

/**
 * The predicate every anchoring rule is gated on, and the negation of the
 * "jump to the end" control's visibility.
 *
 * Only the arithmetic is testable here — the rules that call it need a
 * document and live in `test/e2e/thread-scroll-anchor.spec.ts`. What this pins
 * is the slack: a strict test against zero reads a view that is visibly at the
 * bottom as scrolled away, which stops the transcript following for the rest
 * of the session.
 */
describe("isAtBottom", () => {
  it("is true when the view is exactly at the bottom", () => {
    expect(isAtBottom({ scrollHeight: 1200, scrollTop: 800, clientHeight: 400 })).toBe(true);
  });

  it("is true exactly at the slack boundary", () => {
    expect(
      isAtBottom({ scrollHeight: 1200, scrollTop: 800 - BOTTOM_SLACK_PX, clientHeight: 400 }),
    ).toBe(true);
  });

  it("is false one pixel past the slack boundary", () => {
    expect(
      isAtBottom({ scrollHeight: 1200, scrollTop: 800 - BOTTOM_SLACK_PX - 1, clientHeight: 400 }),
    ).toBe(false);
  });

  it("is false well up the transcript", () => {
    expect(isAtBottom({ scrollHeight: 1200, scrollTop: 0, clientHeight: 400 })).toBe(false);
  });

  it("is true on a fractional clientHeight that never lands on zero", () => {
    expect(
      isAtBottom({ scrollHeight: 1200.5, scrollTop: 800.25, clientHeight: 400.125 }),
    ).toBe(true);
  });

  it("is true for a box with no height yet, before any content has arrived", () => {
    expect(isAtBottom({ scrollHeight: 0, scrollTop: 0, clientHeight: 0 })).toBe(true);
  });
});

describe("readsAsFollowing", () => {
  // A 400px box over 2000px of transcript: the bottom is scrollTop 1600.
  const at = (scrollTop: number) => ({ scrollHeight: 2000, scrollTop, clientHeight: 400 });

  it("follows whenever the view is at the bottom, glide or not", () => {
    expect(readsAsFollowing(at(1600), { gliding: false, previousTop: 1700 })).toBe(true);
    expect(readsAsFollowing(at(1600), { gliding: true, previousTop: 0 })).toBe(true);
  });

  it("keeps following through the pane's own glide, short of the bottom", () => {
    // The glide towards the question just sent, with the reply landing mid-way:
    // every event on the way down is short of the (moving) bottom.
    expect(readsAsFollowing(at(900), { gliding: true, previousTop: 800 })).toBe(true);
    expect(readsAsFollowing(at(900), { gliding: true, previousTop: 900 })).toBe(true);
  });

  it("reads the reader scrolling up during a glide as leaving", () => {
    expect(readsAsFollowing(at(700), { gliding: true, previousTop: 800 })).toBe(false);
  });

  it("reads any stop short of the bottom as leaving when no glide is under way", () => {
    expect(readsAsFollowing(at(900), { gliding: false, previousTop: 800 })).toBe(false);
  });
});
