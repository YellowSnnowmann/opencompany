import { act } from "react";

/**
 * The composer's "Attach files" item, reached the way a person reaches it:
 * open the `+` menu, then find the item. `null` when the composer offers no
 * attaching (no `uploadAttachment`), or when there is no `+` at all.
 *
 * The paperclip was a glyph of its own on the composer's toolbar; it is an item
 * in the one-line composer's `+` menu now, which portals out to
 * `document.body` — so the item is looked up there, not in the container.
 */
export function openAttachItem(container: HTMLElement): HTMLElement | null {
  const trigger = container.querySelector<HTMLButtonElement>('[aria-label="Add to message"]');
  if (!trigger) return null;
  if (trigger.getAttribute("aria-expanded") !== "true") {
    act(() => {
      trigger.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
  }
  return (
    [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((el) =>
      el.textContent?.includes("Attach files"),
    ) ?? null
  );
}

/** Whether a menu item can be chosen — Base UI marks a disabled item rather than disabling a button. */
export function menuItemEnabled(item: HTMLElement): boolean {
  return item.getAttribute("aria-disabled") !== "true" && !item.hasAttribute("data-disabled");
}
