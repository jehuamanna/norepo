// e2e coverage for the "Collasible issues" Archon seed
// (1838a3bd-ac63-4824-8b89-dd12a4015b14):
//   Bug 2 — when the sidebar collapses, the Notes Explorer text must be
//   hidden completely, not just clipped against the activity bar.
//
// Conventions: Test Case Specs (Archon 7094db6c). Tier 4 (Playwright).

import { test, expect } from '../fixtures';

const NOTES_EXPLORER_ICON = '[data-activity-id="notes-explorer:default"]';

test.describe('sidebar collapse', () => {
  test('Ctrl+B hides the Notes Explorer heading completely', async ({ page }) => {
    await page.goto('/');

    const shell = page.locator('#operon-shell');
    await expect(shell).toBeVisible();

    // Activate the Notes Explorer side-bar panel.
    await page.locator(NOTES_EXPLORER_ICON).click();
    const heading = page.getByText('Notes Explorer', { exact: true });
    await expect(heading).toBeVisible();

    await shell.focus();
    await page.keyboard.press('Control+B');

    const sideBar = page.locator('section[data-region="side-bar"]');
    await expect(sideBar).toHaveAttribute('data-collapsed', 'true');

    // The heading must not be visible — even partially.
    await expect(heading).toBeHidden();

    // Sanity: the side-bar grid track has zero (or near-zero) width.
    const sideBarBox = await sideBar.boundingBox();
    expect(sideBarBox?.width ?? 0).toBeLessThanOrEqual(1);
  });

  test('Ctrl+B again restores the sidebar', async ({ page }) => {
    await page.goto('/');
    const shell = page.locator('#operon-shell');
    await expect(shell).toBeVisible();
    await page.locator(NOTES_EXPLORER_ICON).click();
    await expect(page.getByText('Notes Explorer', { exact: true })).toBeVisible();

    await shell.focus();
    await page.keyboard.press('Control+B');
    await page.keyboard.press('Control+B');
    await expect(page.getByText('Notes Explorer', { exact: true })).toBeVisible();
    await expect(page.locator('section[data-region="side-bar"]')).toHaveAttribute(
      'data-collapsed',
      'false',
    );
  });
});
