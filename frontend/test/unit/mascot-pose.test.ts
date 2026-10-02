// @vitest-environment jsdom
//
// The pure half of the per-surface mascot trigger (`lib/mascot-pose.ts`): what
// each trigger means, what makes two looks the same pose, the cache of settled
// frames, the scheduler that keeps first-visit captures from being a dozen
// simultaneous render loops, and which element's hover plays a tile's reaction.
// The Rive runtime is not involved anywhere in here.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { mascotCostumeNumber } from "@/lib/avatar";
import {
  CAPTURE_TIMEOUT_MS,
  DEFAULT_MASCOT_TRIGGER,
  HOVER_SCOPE_ATTRIBUTE,
  MASCOT_HOVER_COSTUME,
  MASCOT_TRIGGERS,
  MAX_ACTIVE_CAPTURES,
  MAX_ACTIVE_HOVER_CAPTURES,
  effectiveMascotTrigger,
  getMascotPose,
  hoverScopeFor,
  mascotPoseKey,
  publishMascotPose,
  requestMascotPoseCapture,
  resetMascotPoses,
  subscribeMascotPose,
  watchHoverScope,
} from "@/lib/mascot-pose";

beforeEach(() => {
  vi.useFakeTimers();
  resetMascotPoses();
});

afterEach(() => {
  resetMascotPoses();
  vi.useRealTimers();
  document.body.innerHTML = "";
});

describe("the trigger vocabulary", () => {
  it("has exactly loop, hover and none, and small tiles default to hover", () => {
    expect([...MASCOT_TRIGGERS]).toEqual(["loop", "hover", "none"]);
    expect(DEFAULT_MASCOT_TRIGGER).toBe("hover");
  });

  it("gives a surface what it asked for when nothing overrides it", () => {
    expect(effectiveMascotTrigger("loop", "animated", false)).toBe("loop");
    expect(effectiveMascotTrigger("hover", "animated", false)).toBe("hover");
    expect(effectiveMascotTrigger("none", "animated", false)).toBe("none");
    expect(effectiveMascotTrigger("hover", undefined, false)).toBe("hover");
  });

  it("a static teammate never animates, whatever the surface asks for", () => {
    for (const asked of MASCOT_TRIGGERS) {
      expect(effectiveMascotTrigger(asked, "static", false)).toBe("none");
    }
  });

  it("reduced motion never animates either", () => {
    for (const asked of MASCOT_TRIGGERS) {
      expect(effectiveMascotTrigger(asked, "animated", true)).toBe("none");
    }
  });
});

describe("a pose key", () => {
  it("is the same for anything that draws the same frame", () => {
    expect(mascotPoseKey(undefined, undefined, undefined)).toBe(mascotPoseKey("cap", "default", "default"));
    // An unrecognised id resolves to the default costume and colors, so it shares their capture.
    expect(mascotPoseKey("not-a-costume", "nope", "nope")).toBe(mascotPoseKey(undefined, undefined, undefined));
  });

  it("differs by costume and by each color", () => {
    const base = mascotPoseKey("cap", "default", "default");
    expect(mascotPoseKey("headphones", "default", "default")).not.toBe(base);
    expect(mascotPoseKey("cap", "coral", "default")).not.toBe(base);
    expect(mascotPoseKey("cap", "default", "forest")).not.toBe(base);
  });

  it("the hover costume is the one behind the fixed hover number", () => {
    expect(mascotCostumeNumber(MASCOT_HOVER_COSTUME)).toBe(2);
  });
});

describe("the pose cache", () => {
  it("has nothing until a frame is published, then serves it", () => {
    expect(getMascotPose("k")).toBeUndefined();
    publishMascotPose("k", "data:image/png;base64,AAA");
    expect(getMascotPose("k")).toBe("data:image/png;base64,AAA");
  });

  it("wakes only the subscribers of the key that arrived", () => {
    const mine = vi.fn();
    const other = vi.fn();
    subscribeMascotPose("mine", mine);
    subscribeMascotPose("other", other);
    publishMascotPose("mine", "x");
    expect(mine).toHaveBeenCalledTimes(1);
    expect(other).not.toHaveBeenCalled();
  });

  it("stops waking a subscriber once it unsubscribes", () => {
    const notify = vi.fn();
    const unsubscribe = subscribeMascotPose("k", notify);
    unsubscribe();
    publishMascotPose("k", "x");
    expect(notify).not.toHaveBeenCalled();
  });
});

describe("the capture scheduler", () => {
  const grantSpy = () => vi.fn();

  it("grants up to the ceiling at once and queues the rest", () => {
    const grants = Array.from({ length: MAX_ACTIVE_CAPTURES + 2 }, grantSpy);
    grants.forEach((grant, i) => requestMascotPoseCapture(`look-${i}`, "rest", grant));
    const granted = grants.filter((g) => g.mock.calls.length > 0).length;
    expect(granted).toBe(MAX_ACTIVE_CAPTURES);
  });

  it("hands a freed slot to the next queued request when a frame is published", () => {
    const grants = Array.from({ length: MAX_ACTIVE_CAPTURES + 1 }, grantSpy);
    grants.forEach((grant, i) => requestMascotPoseCapture(`look-${i}`, "rest", grant));
    const last = grants[MAX_ACTIVE_CAPTURES];
    expect(last).not.toHaveBeenCalled();
    publishMascotPose("look-0", "x");
    expect(last).toHaveBeenCalledTimes(1);
  });

  it("captures one look at a time: a second tile with the same look waits", () => {
    const first = grantSpy();
    const second = grantSpy();
    requestMascotPoseCapture("same", "rest", first);
    requestMascotPoseCapture("same", "rest", second);
    expect(first).toHaveBeenCalledTimes(1);
    expect(second).not.toHaveBeenCalled();
  });

  it("promotes the waiting tile when the capturing one goes away first", () => {
    const first = grantSpy();
    const second = grantSpy();
    const cancelFirst = requestMascotPoseCapture("same", "rest", first);
    requestMascotPoseCapture("same", "rest", second);
    cancelFirst();
    expect(second).toHaveBeenCalledTimes(1);
  });

  it("never grants a waiting tile once its frame has arrived from elsewhere", () => {
    const first = grantSpy();
    const second = grantSpy();
    requestMascotPoseCapture("same", "rest", first);
    requestMascotPoseCapture("same", "rest", second);
    publishMascotPose("same", "x");
    expect(second).not.toHaveBeenCalled();
  });

  it("does not grant a request for a look that is already cached", () => {
    publishMascotPose("cached", "x");
    const grant = grantSpy();
    requestMascotPoseCapture("cached", "rest", grant);
    expect(grant).not.toHaveBeenCalled();
  });

  it("drops a queued request that is cancelled before it is granted", () => {
    const grants = Array.from({ length: MAX_ACTIVE_CAPTURES }, grantSpy);
    grants.forEach((grant, i) => requestMascotPoseCapture(`busy-${i}`, "rest", grant));
    const late = grantSpy();
    const cancel = requestMascotPoseCapture("late", "rest", late);
    cancel();
    publishMascotPose("busy-0", "x");
    expect(late).not.toHaveBeenCalled();
  });

  it("lets a resting frame go ahead of a hover-costume frame", () => {
    // Fill every slot but one, queue a hover request first and a rest request
    // second: the rest request must still be the one to get the free slot.
    for (let i = 0; i < MAX_ACTIVE_CAPTURES - 1; i++) requestMascotPoseCapture(`busy-${i}`, "rest", grantSpy());
    const hover = grantSpy();
    const rest = grantSpy();
    // The last free slot is taken by whichever is granted; use two so one must wait.
    requestMascotPoseCapture("busy-last", "rest", grantSpy());
    requestMascotPoseCapture("hover-look", "hover", hover);
    requestMascotPoseCapture("rest-look", "rest", rest);
    publishMascotPose("busy-0", "x");
    expect(rest).toHaveBeenCalledTimes(1);
    expect(hover).not.toHaveBeenCalled();
  });

  it("keeps the background hover captures to their own small ceiling", () => {
    const grants = Array.from({ length: MAX_ACTIVE_HOVER_CAPTURES + 2 }, grantSpy);
    grants.forEach((grant, i) => requestMascotPoseCapture(`hover-${i}`, "hover", grant));
    expect(grants.filter((g) => g.mock.calls.length > 0).length).toBe(MAX_ACTIVE_HOVER_CAPTURES);
  });

  it("abandons a capture that never produces a frame, and never retries that look", () => {
    const grant = grantSpy();
    const revoke = vi.fn();
    requestMascotPoseCapture("stuck", "rest", grant, revoke);
    expect(grant).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(CAPTURE_TIMEOUT_MS + 1);
    expect(revoke).toHaveBeenCalledTimes(1);
    const retry = grantSpy();
    requestMascotPoseCapture("stuck", "rest", retry);
    expect(retry).not.toHaveBeenCalled();
  });

  it("does not time out a capture that finished in time", () => {
    const revoke = vi.fn();
    requestMascotPoseCapture("ok", "rest", grantSpy(), revoke);
    publishMascotPose("ok", "x");
    vi.advanceTimersByTime(CAPTURE_TIMEOUT_MS * 2);
    expect(revoke).not.toHaveBeenCalled();
  });
});

describe("which element's hover plays a tile's reaction", () => {
  const tileIn = (html: string) => {
    document.body.innerHTML = html;
    return document.querySelector("[data-tile]")!;
  };

  it("is the nearest interactive ancestor — the row, not the 24px face", () => {
    const tile = tileIn('<button id="row"><span><i data-tile></i></span></button>');
    expect(hoverScopeFor(tile).id).toBe("row");
  });

  it("recognises links, list items and option/menuitem/tab roles too", () => {
    expect(hoverScopeFor(tileIn('<a id="s"><i data-tile></i></a>')).id).toBe("s");
    expect(hoverScopeFor(tileIn('<ul><li id="s"><i data-tile></i></li></ul>')).id).toBe("s");
    expect(hoverScopeFor(tileIn('<div role="option" id="s"><i data-tile></i></div>')).id).toBe("s");
    expect(hoverScopeFor(tileIn('<div role="menuitem" id="s"><i data-tile></i></div>')).id).toBe("s");
    expect(hoverScopeFor(tileIn('<div role="tab" id="s"><i data-tile></i></div>')).id).toBe("s");
  });

  it("an explicit marker beats a nearer interactive ancestor, so a whole row can be the target", () => {
    const tile = tileIn(`<article id="row" ${HOVER_SCOPE_ATTRIBUTE}><button><i data-tile></i></button></article>`);
    expect(hoverScopeFor(tile).id).toBe("row");
  });

  it("is the tile itself when nothing encloses it", () => {
    const tile = tileIn('<div><i data-tile></i></div>');
    expect(hoverScopeFor(tile)).toBe(tile);
  });
});

describe("watching the hover scope", () => {
  const setup = () => {
    document.body.innerHTML = '<button id="row"><i data-tile></i></button>';
    const row = document.getElementById("row")!;
    const tile = document.querySelector("[data-tile]")!;
    const onTrigger = vi.fn();
    const stop = watchHoverScope(tile, onTrigger);
    return { row, tile, onTrigger, stop };
  };
  const enter = (el: Element) => el.dispatchEvent(new Event("pointerenter"));
  const leave = (el: Element) => el.dispatchEvent(new Event("pointerleave"));

  it("fires once when the pointer enters the row", () => {
    const { row, onTrigger } = setup();
    enter(row);
    expect(onTrigger).toHaveBeenCalledTimes(1);
  });

  it("does not fire again while the pointer stays, however many enter events arrive", () => {
    const { row, onTrigger } = setup();
    enter(row);
    enter(row);
    enter(row);
    expect(onTrigger).toHaveBeenCalledTimes(1);
  });

  it("fires again after the pointer has left and come back", () => {
    const { row, onTrigger } = setup();
    enter(row);
    leave(row);
    enter(row);
    expect(onTrigger).toHaveBeenCalledTimes(2);
  });

  it("fires for keyboard focus, but not for a click's focus", () => {
    const { row, tile, onTrigger } = setup();
    // jsdom cannot evaluate `:focus-visible`, so the target says whether it is.
    let focusVisible = false;
    Object.defineProperty(tile, "matches", {
      configurable: true,
      value: (q: string) => q === ":focus-visible" && focusVisible,
    });
    tile.dispatchEvent(new Event("focusin", { bubbles: true }));
    expect(onTrigger).not.toHaveBeenCalled();
    focusVisible = true;
    tile.dispatchEvent(new Event("focusin", { bubbles: true }));
    expect(onTrigger).toHaveBeenCalledTimes(1);
    row.dispatchEvent(new Event("focusout"));
  });

  it("stops listening once cleaned up", () => {
    const { row, onTrigger, stop } = setup();
    stop();
    enter(row);
    expect(onTrigger).not.toHaveBeenCalled();
  });
});
