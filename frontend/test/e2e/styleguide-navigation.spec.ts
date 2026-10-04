import { expect, test } from "@playwright/test";

test("the standalone styleguide keeps theme controls and console navigation available", async ({
  page,
}) => {
  await page.addInitScript(() => {
    window.localStorage.setItem("theme", "light");
  });
  await page.goto("/#/styleguide");

  const header = page.getByTestId("styleguide-header");
  const themeToggle = page.getByRole("button", { name: "Change theme" });
  const backToConsole = page.getByRole("link", { name: "Back to console" });

  await expect(header).toBeVisible();
  await expect(themeToggle).toBeVisible();
  await expect(backToConsole).toHaveAttribute("href", "#/chat");

  await page.locator("text=Components").scrollIntoViewIfNeeded();
  const headerBox = await header.boundingBox();
  expect(headerBox?.y).toBe(0);

  await themeToggle.click();
  await page.getByRole("menuitem", { name: "Dark" }).click();
  await expect(page.locator("html")).toHaveClass(/\bdark\b/);

  await backToConsole.click();
  await expect(page).toHaveURL(/#\/chat/);
});
