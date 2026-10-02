import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { Building2 } from "lucide-react";

import { useConsole } from "@/lib/console-context";
import {
  resolveAvatarSrc,
  staticAvatarSrc,
  retainAvatar,
  releaseAvatar,
  blobNodeId,
  subscribeAvatarNode,
  isMascotRef,
} from "@/lib/avatar";
import { DEFAULT_MASCOT_TRIGGER, type MascotTrigger } from "@/lib/mascot-pose";
import { TEAM_TONES, avatarFor, initials } from "@/lib/team";
import { cn } from "@/lib/utils";

/**
 * The Rive runtime + the mascot asset are ~1.8 MB combined — code-split so
 * every tile that is *not* a `mascot:` wearer (the overwhelming majority)
 * pays nothing for it, the same reasoning `agent-profile-sheet.tsx` and
 * `views/team/AgentDetailView.tsx` already apply to their own hero avatars.
 */
const LazyMascotAvatar = lazy(() =>
  import("@/components/mascot-avatar").then((m) => ({ default: m.MascotAvatar })),
);

/**
 * The mascot tile that rests on a settled frame and reacts once on hover — every
 * mascot tile that is not `animate="loop"`. Same chunk as `MascotAvatar` (it
 * imports it), so it costs nothing extra on the wire.
 */
const LazyPoseMascot = lazy(() =>
  import("@/components/mascot-pose").then((m) => ({ default: m.PoseMascot })),
);

interface Props {
  name: string;
  tone?: string;
  /** The company's own voice wears the brand mark rather than initials. */
  company?: boolean;
  /**
   * Draw the tone tile without initials.
   *
   * For decorative stacks small enough that no glyph can be read at a size
   * the tile can hold: a 16px facepile fits two letters only below 10px,
   * and below 10px is not a size, it is a bug. The tile's colour still
   * distinguishes one voice from the next, which is the whole of what a
   * facepile claims to say.
   */
  markOnly?: boolean;
  /**
   * The avatar reference to draw (`lib/avatar.ts`) — `tiny:<flavour>` for a
   * shipped mascot, `blob:<nodeId>` for an image somebody uploaded.
   *
   * Optional: a caller holding a `Member` passes its resolved `avatar` so the
   * face matches everywhere that teammate appears, and a caller with only a name
   * falls back to the mascot hashed from that.
   */
  avatar?: string;
  /**
   * The mascot's chosen costume and colors — from the roster read
   * (`TeamMember`/`TeamMemberDto`), the same values `AgentDetailDto` carries, so
   * a teammate looks the same in a chat gutter as on its own profile sheet.
   * Meaningful only when `avatar` is `"mascot:animated"`; ignored otherwise.
   * Undefined means the file's own default costume/color.
   */
  mascotCostume?: string;
  /** See {@link mascotCostume}. */
  mascotSkinColor?: string;
  /** See {@link mascotCostume}. */
  mascotHandColor?: string;
  /**
   * `"static"` holds the mascot on one pose; anything else (including
   * undefined) is the file's own default, `"animated"`. See
   * `MascotAvatar`'s own `mode` prop for what each does.
   */
  mascotMode?: string;
  /**
   * When a mascot animates on this surface (`MascotTrigger`, `lib/mascot-pose.ts`).
   * Defaults to `"hover"`: a settled frame that plays once when the enclosing
   * row is hovered or focused, and no live canvas at rest — right for the
   * sidebar, chat header, message rows, member lists and the like, where a
   * looping mascot would blank a third of the time and animate in lockstep.
   * `"loop"` keeps a live instance playing; `"none"` is the settled frame only.
   * A static teammate and `prefers-reduced-motion` are always `"none"`. Ignored
   * for an avatar that is not a mascot. The hero surfaces mount `MascotAvatar`
   * directly, which is the loop.
   */
  animate?: MascotTrigger;
  /**
   * This teammate has a turn open right now — passed by the rows that exist
   * exactly while one does (the live receipt, the working row, the typing row).
   * A mascot bobs and wears its replying costume for as long as it is true; it
   * composes with {@link animate}, so `"none"`, a static teammate and reduced
   * motion never reply. Ignored for an avatar that is not a mascot.
   */
  replying?: boolean;
  className?: string;
  /**
   * Forwarded to the tile so a spec can name one avatar among several on a page.
   *
   * Declared rather than picked up from a rest spread: a hyphenated prop passes
   * TypeScript's excess-property check on any component, so an undeclared
   * `data-testid` here would type-check happily and then be dropped at render —
   * a selector that silently matches nothing.
   */
  "data-testid"?: string;
}

/**
 * A square-ish chat avatar: initials on a tone-tinted tile.
 *
 * Rounded rather than circular, which is what distinguishes a workspace
 * avatar from a contact-list one — DM rows, message gutters, and the member
 * pane all draw the same tile at different sizes.
 */
export function TeammateAvatar({
  name,
  tone,
  company,
  markOnly,
  avatar,
  mascotCostume,
  mascotSkinColor,
  mascotHandColor,
  mascotMode,
  animate = DEFAULT_MASCOT_TRIGGER,
  replying,
  className,
  "data-testid": testId,
}: Props) {
  if (company) {
    return (
      <span
        className={cn(
          "flex shrink-0 items-center justify-center rounded-md bg-primary text-primary-foreground",
          className,
        )}
        aria-hidden
        data-testid={testId}
      >
        <Building2 className="size-1/2" />
      </span>
    );
  }

  // `markOnly` is the caller saying "this tile is too small to read". The
  // mascot is subject to the same limit as the initials it replaces — at 16px
  // it is a smudge — so the tone tile stays the answer there rather than a
  // detailed drawing nobody can resolve.
  if (markOnly) {
    return (
      <span
        className={cn(
          "flex shrink-0 items-center justify-center rounded-md text-xs font-semibold",
          toneClass(tone),
          className,
        )}
        aria-hidden
        data-testid={testId}
      />
    );
  }

  return (
    <AvatarTile
      name={name}
      tone={tone}
      avatar={avatar}
      mascotCostume={mascotCostume}
      mascotSkinColor={mascotSkinColor}
      mascotHandColor={mascotHandColor}
      mascotMode={mascotMode}
      animate={animate}
      replying={replying}
      className={className}
      testId={testId}
    />
  );
}

/**
 * The tile itself, once the company-mark and mark-only cases are out of the way.
 *
 * Split out because it holds a hook and the two cases above return before it:
 * a hook cannot sit behind an early return, and hoisting it into the component
 * would mean every 16px facepile tile subscribed to console context and ran an
 * effect to draw nothing.
 */
function AvatarTile({
  name,
  tone,
  avatar,
  mascotCostume,
  mascotSkinColor,
  mascotHandColor,
  mascotMode,
  animate,
  replying,
  className,
  testId,
}: {
  name: string;
  tone?: string;
  avatar?: string;
  mascotCostume?: string;
  mascotSkinColor?: string;
  mascotHandColor?: string;
  mascotMode?: string;
  animate: MascotTrigger;
  replying?: boolean;
  className?: string;
  testId?: string;
}) {
  const ref = avatar ?? avatarFor(name);
  const mascot = isMascotRef(ref);
  // `useAvatarSrc` is still called unconditionally for a mascot reference —
  // hooks cannot sit behind a branch — but it is cheap: `staticAvatarSrc`
  // returns `null` for `mascot:` on purpose (`lib/avatar.ts`), so this never
  // fetches anything for one.
  const src = useAvatarSrc(ref);

  // The tone tile stays underneath the image (or the mascot) on purpose: it
  // is what shows if the avatar 404s, has not loaded yet, or — for a mascot —
  // while its canvas is still loading or the tile is far off-screen
  // (`MascotTile` below).
  return (
    <span
      className={cn(
        "relative flex shrink-0 items-center justify-center overflow-hidden rounded-md text-xs font-semibold",
        toneClass(tone),
        className,
      )}
      aria-hidden
      data-testid={testId}
    >
      <span className="absolute inset-0 flex items-center justify-center">{initials(name)}</span>
      {mascot ? (
        <MascotTile
          costume={mascotCostume}
          skinColor={mascotSkinColor}
          handColor={mascotHandColor}
          mode={mascotMode}
          animate={animate}
          replying={replying}
          className="absolute inset-0 rounded-none"
        />
      ) : (
        // Nothing is drawn until there is something to draw. An uploaded face
        // is fetched through the authenticated client, so its `src` arrives a
        // tick late — rendering an `img` with no source in the meantime would
        // paint the browser's broken-image glyph over the initials this tile
        // is showing precisely so that the gap is never empty.
        src && (
          <img
            src={src}
            alt=""
            loading="lazy"
            decoding="async"
            className="relative size-full object-cover"
          />
        )
      )}
    </span>
  );
}

/**
 * How far past the viewport edge a mascot tile still holds what it draws with,
 * and how long after leaving that range it lets go.
 *
 * A live (`animate="loop"`) tile is one Rive instance (artboard + state machine
 * + render loop); a settled-frame tile holds a frame subscription and, on a
 * cold look, a slot in the capture queue. A long transcript has a tile per
 * message, so the count that matters is "how many are near the screen", not
 * "how many exist" — a tile that has been out of range for a second or two is
 * unmounted and its instance or subscription freed, and remounts (from the
 * shared parsed file and the frame cache, so cheaply) when it comes back. The
 * grace period keeps a tile that is scrolled just past the edge and back from
 * being torn down and rebuilt on every wobble.
 */
const MASCOT_MARGIN_PX = 300;
const MASCOT_RELEASE_MS = 1500;

/**
 * Whether the element behind the returned ref is near the viewport.
 *
 * `IntersectionObserver` measures against every clipping ancestor, so a row
 * scrolled out of the transcript's own scroller is "far" even while it is
 * inside the window's bounds. Without the API (jsdom) everything counts as
 * near, which is what a unit test rendering one tile wants.
 */
function useNearViewport() {
  const ref = useRef<HTMLSpanElement>(null);
  const [near, setNear] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el || typeof IntersectionObserver === "undefined") {
      setNear(true);
      return;
    }
    let release: ReturnType<typeof setTimeout> | undefined;
    const observer = new IntersectionObserver(
      ([entry]) => {
        clearTimeout(release);
        if (entry.isIntersecting) setNear(true);
        else release = setTimeout(() => setNear(false), MASCOT_RELEASE_MS);
      },
      { rootMargin: `${MASCOT_MARGIN_PX}px` },
    );
    observer.observe(el);
    return () => {
      clearTimeout(release);
      observer.disconnect();
    };
  }, []);
  return [ref, near] as const;
}

/**
 * The live mascot for one tile — the same component, with the same look, that
 * every hero surface mounts. Fills its parent (`absolute inset-0` from the
 * caller); until its canvas has loaded (or while the tile is far off-screen)
 * it paints nothing, leaving `AvatarTile`'s initials span as the only thing on
 * screen — the same "never blank" contract its `<img>` branch keeps.
 */
function MascotTile({
  costume,
  skinColor,
  handColor,
  mode,
  animate,
  replying,
  className,
}: {
  costume?: string;
  skinColor?: string;
  handColor?: string;
  mode?: string;
  animate: MascotTrigger;
  replying?: boolean;
  className?: string;
}) {
  const [ref, near] = useNearViewport();
  return (
    <span ref={ref} className={cn("block", className)}>
      {near && (
        <Suspense fallback={null}>
          {animate === "loop" ? (
            <LazyMascotAvatar
              costume={costume}
              skinColor={skinColor}
              handColor={handColor}
              mode={mode}
              state={replying ? "replying" : "idle"}
              className="size-full rounded-none"
            />
          ) : (
            <LazyPoseMascot
              animate={animate}
              replying={replying}
              costume={costume}
              skinColor={skinColor}
              handColor={handColor}
              mode={mode}
              className="size-full rounded-none"
            />
          )}
        </Suspense>
      )}
    </span>
  );
}

/**
 * The `src` for an avatar reference, fetching an uploaded one if that is what it
 * names.
 *
 * A mascot resolves synchronously on the first render — which is what keeps the
 * common case free of a flash — and only a `blob:` reference goes through the
 * client. The fetch is cached module-wide (`resolveAvatarSrc`), so the same
 * uploaded face appearing forty times on a screen costs one request.
 */
function useAvatarSrc(ref: string): string | null {
  const { client, company } = useConsole();
  const immediate = staticAvatarSrc(ref);

  // The URL is stored beside the reference it was fetched for and the scope
  // it was fetched under. A mounted tile whose scope (`client` or `company`)
  // changes while `ref` stays the same — a `blob:` node id that is valid in
  // the previous company — must not keep returning the previous company's
  // object URL; the render that carries the new scope would otherwise answer
  // with the old company's face.
  // A mascot resolves synchronously, so `immediate` is always the current face
  // and the stateful path only ever holds an uploaded one.
  const [fetched, setFetched] = useState<{
    client: typeof client;
    company: typeof company;
    ref: string;
    src: string | null;
  } | null>(null);
  const src =
    fetched?.ref === ref && fetched?.client === client && fetched?.company === company
      ? fetched.src
      : immediate;

  // Revoking a face's object URL does not unpaint a tile that already decoded
  // it, so a delete from the workspace cannot redraw a mounted tile on its own.
  // Subscribing for the node makes `forgetAvatarNode` reach this tile: it bumps
  // `forgot`, which re-runs the resolve below — the deleted bytes 404 and the
  // tile falls back to the tone tile it was already drawing underneath.
  const [forgot, setForgot] = useState(0);
  useEffect(() => {
    const node = blobNodeId(ref);
    if (!node || !client) return;
    return subscribeAvatarNode(client, company, node, () => setForgot((n) => n + 1));
  }, [client, company, ref]);

  useEffect(() => {
    const node = blobNodeId(ref);
    if (!node || !client) return;
    retainAvatar(client, company, node);
    return () => releaseAvatar(client, company, node);
  }, [client, company, ref]);

  useEffect(() => {
    // No client means no authenticated fetch is possible — outside the console
    // shell, or before one is chosen. A mascot still resolves; an uploaded face
    // draws as the tone tile, which is the same thing a deleted one does.
    const resolved = client ? resolveAvatarSrc(client, company, ref) : immediate;
    // The URL is stored beside the scope it was fetched under, so the render
    // guard above can tell a same-`ref` result fetched under the previous scope
    // from one fetched under the current one.
    if (typeof resolved === "string" || resolved === null) {
      setFetched({ client, company, ref, src: resolved });
      return;
    }
    // A reference that changed while a fetch was in flight must not have the
    // stale result written over it — the tile would show the previous person's
    // face, which is worse than showing none.
    let live = true;
    setFetched({ client, company, ref, src: null });
    void resolved.then((url) => {
      if (live) setFetched({ client, company, ref, src: url });
    });
    return () => {
      live = false;
    };
  }, [client, company, ref, immediate, forgot]);

  return src;
}

/** Fall back to a hashed tone so an unnamed voice still gets a stable color. */
function toneClass(tone?: string): string {
  if (tone && TEAM_TONES[tone]) return TEAM_TONES[tone];
  const keys = Object.keys(TEAM_TONES);
  let hash = 0;
  const seed = tone ?? "";
  for (let i = 0; i < seed.length; i++) hash = (hash * 31 + seed.charCodeAt(i)) | 0;
  return TEAM_TONES[keys[Math.abs(hash) % keys.length]];
}
