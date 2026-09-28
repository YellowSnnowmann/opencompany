// A mascot tile that rests on a settled frame and plays once on hover — what
// every `TeammateAvatar` mascot is unless its caller asks for `animate="loop"`
// (`lib/mascot-pose.ts` has the vocabulary and the reasoning).
//
// At rest this is an `<img>`: no canvas, no render loop. The frame is captured
// once per look by a hidden live instance that waits for the pose to settle, then
// shared by every tile with that look. Until it exists the tile draws nothing and
// `AvatarTile`'s initials show through — never a blank, never a half-risen frame.
//
// What "once on hover" is. Mounting a live instance on hover cannot work: a
// fresh one starts below the frame and rises in over ~1.3 s, so the tile would
// blank at the moment the pointer arrives. So the reaction is made from two
// settled frames instead — the tile's own resting frame and the hover
// costume's (the same costume the hero surfaces swap to on hover): a short pop
// while the tile crossfades to the hover costume, holds it briefly, and
// crossfades back. Both frames are cached, so it costs no live canvas and starts
// on the pointer's first frame. If the hover costume's frame has not been
// captured yet (it is captured in the background after the resting frame), only
// the pop plays. A teammate already in the hover costume gets the pop alone.

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { createPortal } from "react-dom";

import { LiveMascot, usePrefersReducedMotion } from "@/components/mascot-avatar";
import {
  MASCOT_HOVER_COSTUME,
  effectiveMascotTrigger,
  getMascotPose,
  mascotPoseKey,
  publishMascotPose,
  requestMascotPoseCapture,
  subscribeMascotPose,
  watchHoverScope,
  type CapturePriority,
  type MascotTrigger,
} from "@/lib/mascot-pose";
import { cn } from "@/lib/utils";

const CROSSFADE_MS = 150;
const HOLD_MS = 700;
/** The whole reaction; a new one cannot start until this has passed. */
const REACTION_MS = CROSSFADE_MS + HOLD_MS + CROSSFADE_MS;
const POP_MS = 420;

/**
 * The frame for a pose key, or `undefined` until it has been captured. `null`
 * means "not wanted", which never subscribes.
 */
function useMascotPose(key: string | null): string | undefined {
  const subscribe = useCallback(
    (notify: () => void) => (key ? subscribeMascotPose(key, notify) : () => {}),
    [key],
  );
  return useSyncExternalStore(
    subscribe,
    () => (key ? getMascotPose(key) : undefined),
    () => undefined,
  );
}

/**
 * Whether this tile has been told to capture `key` right now. Queues a request
 * while `wanted`, and gives the slot back when it stops being wanted (the frame
 * arrived, possibly from another tile) or the tile unmounts.
 */
function useCaptureGrant(key: string, priority: CapturePriority, wanted: boolean): boolean {
  const [granted, setGranted] = useState(false);
  useEffect(() => {
    if (!wanted) return;
    const cancel = requestMascotPoseCapture(
      key,
      priority,
      () => setGranted(true),
      () => setGranted(false),
    );
    return () => {
      cancel();
      setGranted(false);
    };
  }, [key, priority, wanted]);
  return granted && wanted;
}

/**
 * One hidden live instance that waits for its costume to settle and publishes
 * that frame. Portalled to the body at a fixed size inside the viewport: Rive
 * pauses anything geometrically outside it (which yields a transparent
 * capture), and the tile it serves may be 20 px, which would be captured soft
 * and reused at 48.
 */
function PoseCapture({
  poseKey,
  costume,
  skinColor,
  handColor,
}: {
  poseKey: string;
  costume?: string;
  skinColor?: string;
  handColor?: string;
}) {
  if (typeof document === "undefined") return null;
  return createPortal(
    <div aria-hidden className="pointer-events-none fixed bottom-0 left-0 size-24 opacity-0">
      <LiveMascot
        mode="static"
        costume={costume}
        skinColor={skinColor}
        handColor={handColor}
        className="size-full rounded-none"
        onSettled={(url) => publishMascotPose(poseKey, url)}
      />
    </div>,
    document.body,
  );
}

interface Props {
  /** `"hover"` (the default) or `"none"`; `"loop"` is `MascotAvatar`, not this component. */
  animate?: Exclude<MascotTrigger, "loop">;
  /** The teammate's own mode: `"static"` never reacts, whatever `animate` says. */
  mode?: string;
  costume?: string;
  skinColor?: string;
  handColor?: string;
  className?: string;
  "data-testid"?: string;
}

export function PoseMascot({
  animate = "hover",
  mode,
  costume,
  skinColor,
  handColor,
  className,
  "data-testid": testId,
}: Props) {
  const reducedMotion = usePrefersReducedMotion();
  const trigger = effectiveMascotTrigger(animate, mode, reducedMotion);

  const restKey = mascotPoseKey(costume, skinColor, handColor);
  const hoverKey = mascotPoseKey(MASCOT_HOVER_COSTUME, skinColor, handColor);
  const swaps = trigger === "hover" && hoverKey !== restKey;

  const rest = useMascotPose(restKey);
  const hover = useMascotPose(swaps ? hoverKey : null);

  const captureRest = useCaptureGrant(restKey, "rest", rest === undefined);
  // The hover costume waits for the resting frame: the tile is useful without it.
  const captureHover = useCaptureGrant(hoverKey, "hover", swaps && rest !== undefined && hover === undefined);

  const rootRef = useRef<HTMLDivElement>(null);
  const [swapped, setSwapped] = useState(false);
  const busy = useRef(false);
  const timers = useRef<ReturnType<typeof setTimeout>[]>([]);

  const react = useCallback(() => {
    if (busy.current) return;
    busy.current = true;
    const el = rootRef.current;
    if (el && typeof el.animate === "function") {
      el.animate(
        [
          { transform: "scale(1)" },
          { transform: "scale(1.16)", offset: 0.35 },
          { transform: "scale(0.96)", offset: 0.7 },
          { transform: "scale(1)" },
        ],
        { duration: POP_MS, easing: "ease-out" },
      );
    }
    setSwapped(true);
    timers.current.push(
      setTimeout(() => setSwapped(false), CROSSFADE_MS + HOLD_MS),
      setTimeout(() => {
        busy.current = false;
      }, REACTION_MS),
    );
  }, []);

  useEffect(() => {
    const el = rootRef.current;
    if (trigger !== "hover" || rest === undefined || !el) return;
    return watchHoverScope(el, react);
  }, [trigger, rest, react]);

  useEffect(
    () => () => {
      timers.current.forEach(clearTimeout);
    },
    [],
  );

  const showingHover = swapped && hover !== undefined;
  return (
    <div
      ref={rootRef}
      className={cn("relative overflow-hidden rounded-xl", className)}
      data-testid={testId}
      data-mascot-trigger={trigger}
      data-mascot-pose={rest === undefined ? "pending" : showingHover ? "hover" : "rest"}
      aria-hidden
    >
      {rest !== undefined && (
        <img src={rest} alt="" draggable={false} className="absolute inset-0 size-full object-cover" />
      )}
      {hover !== undefined && (
        <img
          src={hover}
          alt=""
          draggable={false}
          className={cn(
            "absolute inset-0 size-full object-cover transition-opacity duration-150",
            showingHover ? "opacity-100" : "opacity-0",
          )}
        />
      )}
      {captureRest && (
        <PoseCapture poseKey={restKey} costume={costume} skinColor={skinColor} handColor={handColor} />
      )}
      {captureHover && (
        <PoseCapture
          poseKey={hoverKey}
          costume={MASCOT_HOVER_COSTUME}
          skinColor={skinColor}
          handColor={handColor}
        />
      )}
    </div>
  );
}
