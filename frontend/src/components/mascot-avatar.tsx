// The animated Rive mascot: a teammate's live canvas. `TeammateAvatar` mounts
// it directly only for the hero surfaces that stay live and react to hover —
// the agent profile sheet, the agent detail page's header, the avatar picker's
// preview. Every small tile is drawn from a *settled frame* instead
// (`components/mascot-pose.tsx`, `lib/mascot-pose.ts`), which is captured by a
// hidden instance of this component and kept across reloads
// (`lib/mascot-pose-store.ts`). See `docs/issue/mascot-profile-avatar/` for the
// deep-dive this was planned from.
//
// Why tiles are not live canvases: every costume rises in over ~1.3 s and most
// duck out of frame about every six seconds, so a dozen live tiles would blink
// in and out, each a running canvas. Why the frame is
// captured only once it has *settled* (`holdPoseOnceSettled`): capturing
// "whatever the canvas shows two frames after the write" raced the entry
// animation and cached blank or half-risen frames. What keeps the live canvases
// that remain cheap is sharing the parsed `.riv` between every instance
// (`useSharedMascotFile`) and only holding one while it is near the viewport.

import { useEffect, useState } from "react";
import {
  type Event as RiveEvent,
  EventType,
  type Rive,
  RiveFile,
  useRive,
  useViewModel,
  useViewModelInstance,
  useViewModelInstanceColor,
  useViewModelInstanceNumber,
} from "@rive-app/react-canvas";

import {
  hexToRgb,
  mascotCostumeNumber,
  mascotHandColorHex,
  mascotSkinColorHex,
  mascotSrc,
  type MascotCostume,
  type MascotHandColor,
  type MascotMode,
  type MascotSkinColor,
} from "@/lib/avatar";
import { contentShare } from "@/lib/mascot-frame";
import { cn } from "@/lib/utils";

export type MascotState = "idle" | "hover" | "replying";

/**
 * The `mascotAnimationNumber` for `hover`/`replying`, kept exactly as the
 * mechanism already built it (confirmed live: `2` swaps the mascot to
 * headphones) rather than reinvented for a chosen costume. A teammate's
 * `idle` baseline is its chosen costume ({@link mascotCostumeNumber}); the
 * reactive states stay these two fixed numbers regardless of that choice —
 * see the module docs on {@link MascotAvatar} for why.
 */
export const REACTIVE_NUMBERS: Record<"hover" | "replying", number> = {
  hover: 2,
  replying: 3,
};

/**
 * The parsed `.riv`, shared by every mascot on the page.
 *
 * `useRive({ src })` makes each instance fetch-and-parse the ~1.7 MB file and
 * decode its embedded raster assets again; with a dozen mascots on screen that
 * is a dozen copies of the same pixels. A `RiveFile` created once and handed to
 * each `useRive({ riveFile })` is parsed once — each instance still gets its
 * own artboard, state machine and ViewModel instance (see the
 * `useViewModelInstance` call below for why that last one must be fresh).
 *
 * Module-level and never released (see the pinned reference below): the file
 * is needed for as long as the page can draw a mascot, which is the life of the
 * tab. A failed load clears the promise so the next mount retries rather than
 * remembering the failure.
 */
let sharedMascotFile: Promise<RiveFile> | null = null;

function loadSharedMascotFile(): Promise<RiveFile> {
  if (!sharedMascotFile) {
    const pending = new Promise<RiveFile>((resolve, reject) => {
      const file = new RiveFile({
        src: mascotSrc("animated"),
        onLoad: () => {
          // Pin it. A `RiveFile` starts at zero references, every instance that
          // uses it adds one (`getInstance`) and gives it back on cleanup, and at
          // zero the runtime releases the native file. Without this permanent
          // reference the file dies the moment the last mascot on a page
          // unmounts, and the next page's mascots get "Problem loading file; may
          // be corrupt!" — which a reload-per-test never shows, because a reload
          // makes a fresh file.
          file.getInstance();
          resolve(file);
        },
        onLoadError: () => reject(new Error("the mascot .riv failed to load")),
      });
      file.init().catch(reject);
    });
    pending.catch(() => {
      if (sharedMascotFile === pending) sharedMascotFile = null;
    });
    sharedMascotFile = pending;
  }
  return sharedMascotFile;
}

function useSharedMascotFile(): RiveFile | null {
  const [file, setFile] = useState<RiveFile | null>(null);
  useEffect(() => {
    let live = true;
    loadSharedMascotFile()
      .then((loaded) => {
        if (live) setFile(loaded);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, []);
  return file;
}

/** Side of the square the canvas is downscaled to for probing; ample for "is anything moving". */
const PROBE_SIZE = 16;

/**
 * A downscaled read of the canvas, or `null` while it has no size yet.
 *
 * Read through a small probe canvas so a probe costs almost nothing however big
 * the tile is.
 */
function readProbe(
  canvas: HTMLCanvasElement,
  probe: HTMLCanvasElement,
  ctx: CanvasRenderingContext2D,
): Uint8ClampedArray | null {
  if (!canvas.width || !canvas.height) return null;
  ctx.clearRect(0, 0, probe.width, probe.height);
  ctx.drawImage(canvas, 0, 0, probe.width, probe.height);
  return ctx.getImageData(0, 0, probe.width, probe.height).data;
}

// `contentShare` lives in `lib/mascot-frame.ts` (the stored-frame check uses it
// too, from the main bundle); re-exported so this module's callers are unchanged.
export { contentShare };

/** Mean per-channel difference (0–255) between two probes: 0 means the frame did not change. */
export function motion(a: Uint8ClampedArray, b: Uint8ClampedArray): number {
  let sum = 0;
  let n = 0;
  for (let i = 0; i < a.length; i += 4) {
    sum += Math.abs(a[i] - b[i]) + Math.abs(a[i + 1] - b[i + 1]) + Math.abs(a[i + 2] - b[i + 2]);
    n += 3;
  }
  return sum / n;
}

/**
 * A settled mascot covers about half its tile (48–61% across all nine costumes
 * and every tile size seen); the rise-in's first stop, a peek, covers only
 * ~21–24%, and a ducked-out frame ~0%.
 */
export const VISIBLE_SHARE = 38;
/**
 * Below this mean per-channel difference between two probes the frame counts as
 * not moving. A costume at rest measures exactly 0.0; the tail of the rise-in
 * measures 5–12.
 */
export const STILL_MOTION = 0.3;
/** Consecutive still-and-visible probes (~0.36 s) it takes to call a pose settled. */
export const STEADY_PROBES = 3;
export const PROBE_EVERY_MS = 120;
/**
 * Never keep a frame before this much *animation* time has elapsed — the
 * rise-in (peek, pop, overshoot, settle) takes ~1.3 s. Counted from Rive's own
 * per-frame `advance` events rather than the wall clock: with many canvases
 * starting at once the main thread stalls and Rive draws nothing for a few
 * hundred ms, while the wall clock keeps running and the mascot has not moved.
 */
export const EARLIEST_KEEP_S = 1.8;
/**
 * Until this much animation time, any visible frame is a valid resting pose:
 * the rise-in is over and the first duck-out (~4 s) has not started. After it,
 * a visible frame might be the mascot half-way out of or into frame, so only a
 * *still* one is trusted. (Stillness cannot be the rule from the start: two of
 * the nine costumes, headphones and glass1, have a perpetual small bob and are
 * never still.)
 */
export const QUIET_WINDOW_END_S = 3.5;
/** Give up freezing (and just keep playing) rather than keep a frame that never settled visibly. */
const GIVE_UP_AFTER_MS = 12_000;

/**
 * Whether this probe's frame is one worth keeping as the mascot's still pose.
 *
 * `animatedFor` is seconds of Rive animation time, `visible` whether the mascot
 * is clearly on screen, `steady` how many consecutive probes have been visible
 * and not moving. The three regions this encodes were each measured, not
 * assumed: before {@link EARLIEST_KEEP_S} the mascot is still rising in (its
 * first stop, a peek, is even *steady* — at ~24% coverage); from there until
 * {@link QUIET_WINDOW_END_S} every visible frame is a fine resting pose, which
 * is the only rule that works for the two costumes that never stop bobbing;
 * after it a duck-out may be under way, so only a frame that has held still
 * for {@link STEADY_PROBES} probes is trusted.
 */
export function shouldKeepFrame(animatedFor: number, visible: boolean, steady: number): boolean {
  if (!visible || animatedFor < EARLIEST_KEEP_S) return false;
  return animatedFor < QUIET_WINDOW_END_S || steady >= STEADY_PROBES;
}

/**
 * Calls `onSettled` once, with the canvas, on the first frame where the mascot
 * is on screen and has stopped moving. Returns the cancel function.
 *
 * The file is not a set of stills. Every costume plays a rise-in on load —
 * peek, pop, overshoot, settle — and then an idle loop that ducks the mascot
 * out of frame and back roughly every six seconds. "Hold one pose" therefore
 * cannot mean "stop after N ms" or "grab whatever is on the canvas": either
 * lands on a hidden, half-risen or still-zooming frame. It means waiting for a
 * frame that is clearly visible ({@link VISIBLE_SHARE}), after the rise-in
 * ({@link EARLIEST_KEEP_S}) and — once the first duck-out could have begun
 * ({@link QUIET_WINDOW_END_S}) — stopped moving ({@link STILL_MOTION}), and then
 * keeping *that frame*.
 * Pausing the runtime on it is not the same thing: a paused instance redraws
 * differently from the one that was playing (every costume with a duck-out
 * loop came back scaled and cropped), so the caller copies the pixels out and
 * lets the instance go.
 */
function holdPoseOnceSettled(
  rive: Rive,
  canvas: HTMLCanvasElement,
  onSettled: (canvas: HTMLCanvasElement) => void,
): () => void {
  const probe = document.createElement("canvas");
  probe.width = PROBE_SIZE;
  probe.height = PROBE_SIZE;
  const ctx = probe.getContext("2d", { willReadFrequently: true });
  if (!ctx) return () => {};
  // Rive's own clock: seconds advanced since we started.
  let animatedFor = 0;
  const onAdvance = (event: RiveEvent) => {
    animatedFor += typeof event.data === "number" ? event.data : 0;
  };
  rive.on(EventType.Advance, onAdvance);
  const started = performance.now();
  let previous: Uint8ClampedArray | null = null;
  let steady = 0;
  const timer = setInterval(() => {
    if (performance.now() - started > GIVE_UP_AFTER_MS) {
      clearInterval(timer);
      // No settled frame arrived. A frozen (static / reduced-motion) mascot
      // must not fall back to animating without end, so hold whatever frame
      // is on screen: an imperfect still beats motion the viewer opted out of.
      rive.pause();
      return;
    }
    const current = readProbe(canvas, probe, ctx);
    if (!current) return;
    const visible = contentShare(current) >= VISIBLE_SHARE;
    const still = previous !== null && motion(previous, current) <= STILL_MOTION;
    previous = new Uint8ClampedArray(current);
    steady = still && visible ? steady + 1 : 0;
    if (shouldKeepFrame(animatedFor, visible, steady)) {
      clearInterval(timer);
      onSettled(canvas);
    }
  }, PROBE_EVERY_MS);
  return () => {
    clearInterval(timer);
    rive.off(EventType.Advance, onAdvance);
  };
}

interface Props {
  /**
   * Whether the mascot keeps moving. Defaults to `"animated"`, the file's own
   * default: the costume's rise-in, then its idle loop (the mascot ducks out
   * of frame and back about every six seconds), and the hover/replying swaps.
   * `"static"` holds one pose — the first frame where the mascot is on screen
   * and still (`holdPoseOnceSettled`), kept as an image — and {@link Props.state} is ignored
   * entirely; a static mascot does not react to hover or "replying", so callers
   * should not wire those handlers up for it either.
   */
  mode?: MascotMode | string;
  /**
   * Which of the file's states to play. Defaults to idle. Ignored in
   * `"static"` mode. In `"animated"` mode, `hover`/`replying` are the fixed
   * {@link REACTIVE_NUMBERS} already built — they do not swap to a different
   * costume than the one chosen; only `idle` lands on it.
   */
  state?: MascotState;
  /**
   * The chosen costume id (`MASCOT_COSTUMES` in `lib/avatar.ts`), or
   * `undefined` for the file's own default (`"cap"`). Applies in both display
   * modes: the resting frame in `"static"`, the `idle` baseline in
   * `"animated"`. An unrecognised id falls back to the default rather than
   * crashing the Rive ViewModel write.
   */
  costume?: MascotCostume | string;
  /** The chosen skin (body) color id, or `undefined` for the file's own default. */
  skinColor?: MascotSkinColor | string;
  /** The chosen hand/accent color id, or `undefined` for the file's own default. */
  handColor?: MascotHandColor | string;
  className?: string;
  "data-testid"?: string;
}

interface LiveProps extends Props {
  /**
   * Called once, with a PNG of the settled pose, when a frozen (static or
   * reduced-motion) mascot has reached it. Never called for an animated one.
   */
  onSettled?: (dataUrl: string) => void;
}

/**
 * `prefers-reduced-motion: reduce` holds the mascot on its idle frame.
 *
 * The same accessibility carve-out an animated GIF avatar already has to
 * consider (`docs/spec/runtime/avatars.md`) — "a moving face is more
 * recognisable" assumes the viewer can tolerate motion, which reduced-motion
 * says they can't. Duplicated locally rather than shared: three other
 * components in this codebase (`WorkingIndicator`, `ChatLiveReceipt`,
 * `RevealSelectedNode`) already each keep their own copy of this exact hook
 * rather than a shared one, so this follows the established convention.
 */
export function usePrefersReducedMotion(): boolean {
  // Read on the first render, not only in the effect: a reduced-motion viewer's
  // first commit must already honour the preference, or the bob would start and
  // only be cancelled on the second commit.
  const [reduced, setReduced] = useState(
    () =>
      typeof window !== "undefined" &&
      (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false),
  );
  useEffect(() => {
    const mql = window.matchMedia?.("(prefers-reduced-motion: reduce)");
    if (!mql) return;
    setReduced(mql.matches);
    const onChange = () => setReduced(mql.matches);
    if (typeof mql.addEventListener === "function") {
      mql.addEventListener("change", onChange);
      return () => mql.removeEventListener("change", onChange);
    }
    mql.addListener(onChange);
    return () => mql.removeListener(onChange);
  }, []);
  return reduced;
}

/** One live Rive instance. Everything that touches the runtime lives here. */
export function LiveMascot({
  mode = "animated",
  state = "idle",
  costume,
  skinColor,
  handColor,
  className,
  "data-testid": testId,
  onSettled,
}: LiveProps) {
  const reducedMotion = usePrefersReducedMotion();
  const isStatic = mode === "static";
  // Static, and reduced motion, both hold one pose (see `holdPoseOnceSettled`).
  const frozen = isStatic || reducedMotion;
  const riveFile = useSharedMascotFile();
  // `null` until the shared file has parsed: the hook then creates no instance
  // at all, and `RiveComponent` below still renders its (empty) canvas, so the
  // caller's placeholder shows through exactly as it did while a per-instance
  // `src` was loading.
  const riveParams = riveFile
    ? {
        riveFile,
        // The file has one *loadable* artboard, literally named "Artboard" —
        // `useRive({ artboard: "Mascot" })` throws "Invalid artboard name or no
        // default artboard", so `Mascot Instance` (seen in the object graph) is
        // a node inside `Artboard`, not a separately loadable artboard; there is
        // no nested artboard to route around. This omits `artboard` and lets the
        // runtime use its default.
        //
        // `Artboard` carries three state machines (`rive.stateMachineNames`):
        // `MascotProfileAnimations`, `animtionStatemachin`, and `State Machine
        // 1`. The Rive editor's Data panel shows `mascotAnimationNumber` bound
        // under `State Machine 1`, which is what an earlier pass loaded here —
        // the ViewModel write round-tripped through its own getter, but the
        // rendered artboard never moved (canvas-pixel sampling, byte-identical
        // across states). `MascotProfileAnimations` is the one that actually
        // drives the costume swap: loading *this* one instead, with the same
        // ViewModel writes below unchanged, visibly swaps the mascot's cap for
        // headphones on hover (confirmed both by pixel sampling and a
        // screenshot). Both machines can apparently read the same bound
        // ViewModel instance; only one of them acts on it. See
        // `docs/issue/mascot-profile-avatar/open-questions.md` §3.
        //
        // `autoBind: true` lets the runtime perform its own default
        // ViewModel-instance binding at load time, ahead of this component's own
        // manual `useViewModel`/`useViewModelInstance` calls below.
        stateMachine: "MascotProfileAnimations",
        autoBind: true,
        // Always `true`, including for a static or reduced-motion mascot.
        // `autoplay: false` does not mean "paint one frame and stop" — it means
        // the Rive runtime never starts its render loop at all, so the canvas
        // never paints *anything*, including the ViewModel-driven costume/color
        // writes below (confirmed live: a static mascot rendered fully
        // transparent, and so did reduced motion). Holding a pose is done by
        // keeping an already-painted, settled frame — see `holdPoseOnceSettled`
        // and `MascotAvatar` — not by never starting.
        autoplay: true,
      }
    : null;
  const { rive, canvas, RiveComponent } = useRive(riveParams);

  const viewModel = useViewModel(rive, { useDefault: true });
  // A fresh instance per mascot, not the ViewModel's default one: with a shared
  // file, `useDefault` hands every canvas the *same* instance, so one
  // teammate's `skinColor` write would repaint every other mascot on the page.
  const vmi = useViewModelInstance(viewModel, { useNew: true, rive });

  const { setValue: setAnimationNumber } = useViewModelInstanceNumber(
    "mascotAnimationNumber",
    vmi,
  );
  const { setRgb: setHandColor } = useViewModelInstanceColor("handColor", vmi);
  const { setRgb: setSkinColor } = useViewModelInstanceColor("skinColor", vmi);

  // Whether the canvas has actually been given its chosen costume and colors
  // at least once. `vmi` becoming truthy only means the file loaded and the
  // ViewModel bound — the canvas itself keeps painting the file's own default
  // (uncostumed) frame until the effects below write to it, which is a tick
  // later. Gating visibility on this instead of on `vmi`/`rive` directly is
  // what prevents a mount from ever showing the wrong costume, even for one
  // frame, and — combined with staying transparent until then — is what lets
  // a caller layer this over its own placeholder (`AvatarTile`'s tone tile,
  // or a caller's own `Skeleton`) without that placeholder being replaced by
  // a flash of the un-costumed default first.
  const [ready, setReady] = useState(false);

  // The chosen colors, or the file's own defaults when unset or
  // unrecognised. Set whenever either changes — not just once — so a picker
  // preview updates live as an operator tries different swatches before
  // saving. Applies identically in both display modes.
  useEffect(() => {
    if (!vmi) return;
    setHandColor(...hexToRgb(mascotHandColorHex(handColor)));
    setSkinColor(...hexToRgb(mascotSkinColorHex(skinColor)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [vmi, handColor, skinColor]);

  // The costume baseline, and — in animated mode only — the hover/replying
  // swap already built. A static mascot ignores `state` entirely: it freezes
  // on the chosen costume and never re-fires this effect for a state change,
  // because reduced-motion and static both resolve to the same baseline
  // number regardless of what `state` says.
  useEffect(() => {
    if (!vmi) return;
    const baseline = mascotCostumeNumber(costume);
    const number =
      isStatic || reducedMotion || state === "idle"
        ? baseline
        : REACTIVE_NUMBERS[state];
    setAnimationNumber(number);
    setReady(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [vmi, isStatic, state, reducedMotion, costume]);

  // Hold the pose for a static / reduced-motion mascot: once it has settled,
  // hand the parent a PNG of that exact frame. Declared after the effects above
  // so their ViewModel writes have landed first.
  useEffect(() => {
    if (!frozen || !rive || !vmi || !canvas || !onSettled) return;
    return holdPoseOnceSettled(rive, canvas, (settled) => {
      try {
        onSettled(settled.toDataURL("image/png"));
      } catch {
        // A canvas Rive draws into procedurally is never cross-origin-tainted,
        // but `toDataURL` is specified to throw if it is. Staying live is
        // the safe failure: the mascot stays on screen rather than vanishing,
        // paused so a frozen mascot does not animate without end.
        rive.pause();
      }
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rive, canvas, vmi, frozen, costume, skinColor, handColor]);

  return (
    <div
      className={cn("overflow-hidden rounded-xl", className)}
      data-testid={testId}
      aria-hidden
    >
      {/* The `.riv` file is ~1.7 MB and can take a couple of seconds to fetch
          and instance even after this component's own chunk has loaded, and
          an untouched canvas is fully transparent — painting nothing at all
          is what read as "broken" (issue found live 2026-09-26). Opacity
          rather than an unmount keeps `RiveComponent` mounted (and its
          `autoplay` loop warm) throughout, so there is no second mount cost
          once the file lands; only its visibility changes. Fully transparent
          until `ready` means whatever a caller has placed behind this
          (`AvatarTile`'s initials tile, or a caller's own `Skeleton`) is what
          shows during the gap, instead of this component inventing its own
          placeholder that would paint over — and hide — that one. */}
      <RiveComponent
        className={cn(
          // `RiveComponent` renders its own wrapper around the actual
          // `<canvas>` rather than spreading straight onto it (confirmed by
          // inspecting the mounted DOM: `className` here lands on that
          // wrapper, not the canvas). That wrapper has no size of its own —
          // it is sized by its content — and the canvas in turn sizes itself
          // to *its* parent (`shouldResizeCanvasToContainer`'s default),
          // which is this very wrapper. With neither given an explicit size,
          // that is a circular "auto" on both ends and the canvas resolves
          // to a real width but a **zero height** (confirmed live: `<canvas
          // width="56" height="0">`) — painting nothing, silently, no error
          // anywhere. `size-full` breaks the circle: it is a definite size
          // (100% of the outer `div` above, which this component's own
          // `className` prop already sizes, one way or another, at every
          // call site). Found live 2026-09-26 against the agent detail
          // page's hero avatar, after the mascot had rendered correctly
          // everywhere it was tested up to this point.
          "size-full",
          "transition-opacity duration-150",
          ready ? "opacity-100" : "opacity-0",
        )}
      />
    </div>
  );
}

/**
 * The mascot. One component for every surface that draws a `mascot:animated`
 * teammate — `TeammateAvatar` mounts it for every tile, the hero surfaces mount
 * it directly (they also drive `state` from hover).
 *
 * An animated mascot is a live Rive instance. A static one (or any mascot under
 * `prefers-reduced-motion`) plays just long enough to reach its settled pose,
 * then becomes a plain `<img>` of that frame and the instance is released — so a
 * screenful of static teammates costs no live canvases at all. Changing the
 * costume, either color, or the mode starts it over.
 *
 * Callers should `lazy()`-load this module (mirroring the
 * `lazy(() => import(...).then((m) => ({ default: m.X })))` convention this
 * codebase already uses for `recharts`/`@xyflow/react`/`react-joyride`) rather
 * than importing `@rive-app/react-canvas` directly — this file is the
 * code-split boundary.
 */
export function MascotAvatar(props: Props) {
  const reducedMotion = usePrefersReducedMotion();
  const frozen = props.mode === "static" || reducedMotion;
  const { costume, skinColor, handColor } = props;
  const [settledUrl, setSettledUrl] = useState<string | null>(null);

  useEffect(() => {
    setSettledUrl(null);
  }, [frozen, costume, skinColor, handColor]);

  if (frozen && settledUrl) {
    return (
      <div
        className={cn("overflow-hidden rounded-xl", props.className)}
        data-testid={props["data-testid"]}
        aria-hidden
      >
        <img src={settledUrl} alt="" className="size-full object-cover" />
      </div>
    );
  }
  return <LiveMascot {...props} onSettled={frozen ? setSettledUrl : undefined} />;
}
