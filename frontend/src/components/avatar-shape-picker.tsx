import type { CSSProperties } from "react";
import { RadioGroup } from "@base-ui/react/radio-group";
import { Radio } from "@base-ui/react/radio";

import { TeammateAvatar } from "@/components/teammate-avatar";
import {
  AVATAR_SHAPES,
  setAvatarShape,
  useAvatarShape,
  type AvatarShape,
} from "@/lib/avatar-shape";

/**
 * Each option's own radius, so its preview shows the shape it would choose
 * rather than the one currently applied — the same values `index.css` keys on
 * `data-avatar-shape`.
 */
const PREVIEW_RADIUS: Record<AvatarShape, string> = {
  round: "9999px",
  rounded: "calc(var(--radius) - 2px)",
};

/**
 * The avatar-shape picker on Settings → Appearance: a `role="radiogroup"` of
 * two options, each previewing a teammate face in its shape with its name as
 * visible text. Built on the same Base UI radio primitive as the accent picker,
 * which supplies arrow-key roving focus and `aria-checked`.
 */
export function AvatarShapePicker() {
  const current = useAvatarShape();
  return (
    <RadioGroup
      aria-label="Avatar shape"
      value={current}
      onValueChange={(value) => setAvatarShape(value as AvatarShape)}
      className="flex flex-wrap gap-2"
    >
      {AVATAR_SHAPES.map((shape) => (
        <Radio.Root
          key={shape.id}
          value={shape.id}
          className="flex items-center gap-3 rounded-lg border px-3 py-2 text-sm outline-none transition-colors hover:bg-accent/40 focus-visible:ring-3 focus-visible:ring-ring/50 data-checked:border-primary data-checked:bg-accent/40"
        >
          <span
            className="flex items-center"
            style={{ "--avatar-radius": PREVIEW_RADIUS[shape.id] } as CSSProperties}
          >
            <TeammateAvatar name="Ada" className="size-9 text-xs" />
          </span>
          {shape.label}
        </Radio.Root>
      ))}
    </RadioGroup>
  );
}
