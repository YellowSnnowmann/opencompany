import { expect, test, type Page } from "@playwright/test";

/**
 * The sidebar is the conversation list on top and Company / Connections pinned
 * at the foot, and the foot lines up with the console card.
 *
 * Measured in a real browser, because both claims are about layout: the list
 * must scroll INSIDE itself (a flex item defaults to `min-height: auto`, so one
 * missing `min-h-0` and the rows are pushed off the bottom instead), and the
 * last row's bottom edge must be the card's bottom edge (the shell's
 * `--frame-inset` gap, reused rather than a second number).
 */

async function dismissTour(page: Page) {
  const skip = page.getByRole("button", { name: "Skip for now" });
  try {
    await skip.waitFor({ state: "visible", timeout: 10_000 });
    await skip.click();
  } catch {
    // The profile may already have completed the tour.
  }
  await expect(page.locator('[data-slot="dialog-overlay"]')).toHaveCount(0);
}

test.describe("sidebar conversations layout", () => {
  test("keeps Company and Connections in view when the list overflows", async ({ page }) => {
    // Short enough that the harness company's rows cannot fit above the pair.
    await page.setViewportSize({ width: 1280, height: 420 });
    await page.goto("/#/company");
    await dismissTour(page);

    const slot = page.getByTestId("room-rail-slot");
    await expect(slot.locator("li").first()).toBeVisible();
    const overflow = await slot.evaluate((el) => ({
      scroll: el.scrollHeight,
      client: el.clientHeight,
      overflowY: getComputedStyle(el).overflowY,
    }));
    expect(overflow.scroll, "the list overflows its slot").toBeGreaterThan(overflow.client);
    expect(overflow.overflowY).toBe("auto");

    const viewport = page.viewportSize()!;
    for (const name of ["Company", "Connections"]) {
      const row = page.locator(`[data-tour="nav-${name.toLowerCase()}"]`);
      await expect(row).toBeInViewport({ ratio: 1 });
      const box = (await row.boundingBox())!;
      expect(box.y + box.height, `${name} is above the window's foot`).toBeLessThanOrEqual(
        viewport.height,
      );
    }
    // Company first, Connections last: the order the operator reads them in.
    const company = (await page.locator('[data-tour="nav-company"]').boundingBox())!;
    const connections = (await page.locator('[data-tour="nav-connections"]').boundingBox())!;
    expect(company.y).toBeLessThan(connections.y);
  });

  test("lands Connections' bottom edge on the console card's bottom edge", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto("/#/company");
    await dismissTour(page);

    const edges = await page.evaluate(() => {
      const bottom = (el: Element | null) => el!.getBoundingClientRect().bottom;
      const card = document.querySelector("[data-slot=sidebar-inset]")!.firstElementChild;
      const button = document.querySelector('[data-tour="nav-connections"] button');
      return { card: bottom(card), connections: bottom(button) };
    });
    expect(edges.connections).toBeCloseTo(edges.card, 0);
  });

  test("renders no Room or Automations row and no Channels / Direct messages caption", async ({
    page,
  }) => {
    await page.goto("/#/company");
    await dismissTour(page);
    const nav = page.getByRole("navigation", { name: "Main navigation", exact: true });
    await expect(nav.getByText("Conversations", { exact: true })).toBeVisible();
    await expect(nav.getByText("Channels", { exact: true })).toHaveCount(0);
    await expect(nav.getByText("Direct messages", { exact: true })).toHaveCount(0);
    for (const name of ["Room"]) {
      await expect(nav.getByRole("button", { name, exact: true })).toHaveCount(0);
    }
  });

  test("opens Automations from Company's rail with Company lit in the sidebar", async ({
    page,
  }) => {
    await page.goto("/#/company");
    await dismissTour(page);
    await page.locator('[data-tour="nav-workflows"]').getByRole("button").first().click();
    await expect(page).toHaveURL(/#\/workflows/);
    await expect(
      page.locator('[data-tour="nav-company"] [data-sidebar=menu-button]'),
    ).toHaveAttribute("data-active", "");
  });

  test.describe("the two doors on the Conversations row", () => {
    // Each door is a menu, and each item opens a different dialog. They are told
    // apart by what is IN them, not by "a dialog opened": `Create a new agent`
    // once opened the agent picker ("New message") instead of the Add agent
    // form, and a bare dialog-is-visible check passes for both.
    async function openAt(page: Page) {
      await page.setViewportSize({ width: 1440, height: 900 });
      await page.goto("/#/company");
      await dismissTour(page);
    }

    const door = (page: Page, name: string) =>
      page.getByRole("button", { name, exact: true });
    const items = (page: Page) => page.getByRole("menuitem");
    const dialog = (page: Page) => page.getByRole("dialog");

    async function closeDialog(page: Page) {
      await page.keyboard.press("Escape");
      await expect(dialog(page)).toHaveCount(0);
    }

    test("the + menu offers exactly Create a new channel and Create a new agent", async ({
      page,
    }) => {
      await openAt(page);
      await door(page, "New").click();
      await expect(items(page)).toHaveText(["Create a new channel", "Create a new agent"]);
    });

    test("Create a new channel opens the New channel form", async ({ page }) => {
      await openAt(page);
      await door(page, "New").click();
      await page.getByRole("menuitem", { name: "Create a new channel" }).click();

      const form = dialog(page);
      await expect(form.getByText("New channel", { exact: true })).toBeVisible();
      await expect(form.getByText("Name", { exact: true })).toBeVisible();
      await expect(form.getByText("What it's for", { exact: true })).toBeVisible();
      await expect(form.getByText("Members", { exact: true })).toBeVisible();
      // Not the agent picker.
      await expect(form.getByText("New message", { exact: true })).toHaveCount(0);
      await closeDialog(page);
    });

    test("Create a new agent opens the Add agent form, not the New message picker", async ({
      page,
    }) => {
      await openAt(page);
      await door(page, "New").click();
      await page.getByRole("menuitem", { name: "Create a new agent" }).click();

      const form = dialog(page);
      // The real create form: a name, an icon and a post.
      await expect(form.getByText("Add agent", { exact: true }).first()).toBeVisible();
      await expect(form.getByPlaceholder("e.g. Ada")).toBeVisible();
      await expect(form.getByText("Icon", { exact: true })).toBeVisible();
      await expect(form.getByText("Post", { exact: true })).toBeVisible();
      // And what the regression looked like: the DM picker.
      await expect(form.getByText("New message", { exact: true })).toHaveCount(0);
      await expect(
        form.getByText("Choose an agent to start a direct message."),
      ).toHaveCount(0);
      await closeDialog(page);
    });

    test("the pencil menu offers the two Start a conversation items and no Search", async ({
      page,
    }) => {
      await openAt(page);
      await door(page, "Start a conversation").click();
      await expect(items(page)).toHaveText([
        "Start a conversation in a channel",
        "Start a conversation with the agent",
      ]);
      await expect(page.getByRole("menuitem", { name: /search/i })).toHaveCount(0);
    });

    test("the channel picker lists channels and the agent picker lists agents", async ({
      page,
    }) => {
      await openAt(page);

      await door(page, "Start a conversation").click();
      await page.getByRole("menuitem", { name: "Start a conversation in a channel" }).click();
      const channels = dialog(page);
      await expect(
        channels.getByText("Start a conversation in a channel", { exact: true }),
      ).toBeVisible();
      await expect(channels.getByText("Choose a channel to talk in.")).toBeVisible();
      // The first row of the rail is a channel (channels come first), so the
      // picker must list it by name — and no agent.
      const firstChannel = (
        await page
          .getByTestId("room-rail-slot")
          .locator("li button span.truncate")
          .first()
          .innerText()
      ).trim();
      await expect(channels.getByRole("button", { name: new RegExp(`^${firstChannel}`) })).toBeVisible();
      await expect(channels.getByRole("button", { name: /^Chief Executive/ })).toHaveCount(0);
      await closeDialog(page);

      await door(page, "Start a conversation").click();
      await page.getByRole("menuitem", { name: "Start a conversation with the agent" }).click();
      const agents = dialog(page);
      await expect(agents.getByText("New message", { exact: true })).toBeVisible();
      await expect(agents.getByText("Choose an agent to start a direct message.")).toBeVisible();
      await expect(agents.getByRole("button", { name: /^Chief Executive/ })).toBeVisible();
      await expect(agents.getByRole("button", { name: new RegExp(`^${firstChannel}`) })).toHaveCount(0);
      await closeDialog(page);
    });
  });
});
