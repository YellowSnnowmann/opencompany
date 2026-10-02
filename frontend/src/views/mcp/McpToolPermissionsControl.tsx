import { useRef } from "react";

import type { ApprovalMode } from "@/api/mcp-tool-policy";
import { cn } from "@/lib/utils";

/** What each mode does, in the words the operator is choosing between. */
export const MODE_LABELS: Record<ApprovalMode, string> = {
  always_allow: "Allow",
  needs_approval: "Needs approval",
  blocked: "Block",
};

const MODES = Object.keys(MODE_LABELS) as ApprovalMode[];

const ARROWS = ["ArrowDown", "ArrowRight", "ArrowUp", "ArrowLeft"];

interface Props {
  value: ApprovalMode;
  label: string;
  disabled: boolean;
  /**
   * Modes this control may show but not set, each with the reason. The reason is
   * the option's `title`, so it is reachable by pointer and by accessible
   * description.
   */
  disabledModes?: Partial<Record<ApprovalMode, string>>;
  onChange: (mode: ApprovalMode) => void;
}

export function ModeChoice({
  value,
  label,
  disabled,
  disabledModes,
  onChange,
}: Props) {
  const radios = useRef<(HTMLButtonElement | null)[]>([]);
  const refused = (mode: ApprovalMode) => disabledModes?.[mode] !== undefined;

  function handleKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
    if (disabled || !ARROWS.includes(event.key)) return;
    const step =
      event.key === "ArrowDown" || event.key === "ArrowRight" ? 1 : -1;
    const focused = radios.current.indexOf(event.target as HTMLButtonElement);
    if (focused === -1) return;
    event.preventDefault();
    // Arrow keys walk past a mode this scope may not set.
    let next = focused;
    for (let hop = 0; hop < MODES.length; hop += 1) {
      next = (next + step + MODES.length) % MODES.length;
      const candidate = MODES[next];
      if (candidate && !refused(candidate)) break;
    }
    const mode = MODES[next];
    if (!mode || refused(mode)) return;
    radios.current[next]?.focus();
    if (mode !== value) onChange(mode);
  }

  return (
    <div
      role="radiogroup"
      aria-label={label}
      className="inline-flex shrink-0 rounded-md border border-border p-0.5"
      onKeyDown={handleKeyDown}
    >
      {MODES.map((mode, index) => {
        const active = mode === value;
        const why = disabledModes?.[mode];
        return (
          <button
            key={mode}
            ref={(el) => {
              radios.current[index] = el;
            }}
            type="button"
            role="radio"
            aria-checked={active}
            tabIndex={active ? 0 : -1}
            disabled={disabled || why !== undefined}
            title={why}
            data-testid={`mcp-mode-${mode}`}
            onClick={() => {
              if (!active) onChange(mode);
            }}
            className={cn(
              "rounded px-2 py-0.5 text-xs font-medium transition-colors",
              "disabled:cursor-not-allowed disabled:opacity-60",
              active
                ? "bg-secondary text-secondary-foreground"
                : "text-muted-foreground hover:bg-muted",
            )}
          >
            {MODE_LABELS[mode]}
          </button>
        );
      })}
    </div>
  );
}
