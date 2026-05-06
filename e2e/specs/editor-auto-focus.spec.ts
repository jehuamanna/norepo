// Plans-Phase-2-editor-auto-focus + Phase-7 follow-ups.
//
// Covers TestCase-Phase-2 E2E-1..6 plus Phase-7's tab-activation
// assertion. Marked `test.skip` until the Phase-2 wasm Store + Monaco
// bridge load reliably in `dx serve --platform web` (matches the
// existing pattern in note-create.spec.ts).

import { test, expect } from '@playwright/test';

test.describe.skip('Phase 2 + 7 — editor auto-focus', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/?test=1');
  });

  test('E2E-1 — clicking an unopened note focuses Monaco', async ({ page }) => {
    const row = page.locator('[data-testid="note-row"]').first();
    await row.click();
    // Wait for the Monaco mount to settle (≤ 500 ms after rAF).
    await expect.poll(async () =>
      page.evaluate(() =>
        document.activeElement?.classList.contains('monaco-editor') ?? false,
      ),
    ).toBe(true);
  });

  test('E2E-2 — re-clicking an already-open tab re-focuses Monaco', async ({ page }) => {
    const rows = page.locator('[data-testid="note-row"]');
    await rows.nth(0).click();
    await rows.nth(1).click();
    // Click the first row again; expect activeElement to be the first
    // tab's Monaco instance.
    await rows.nth(0).click();
    await expect.poll(async () =>
      page.evaluate(() =>
        document.activeElement?.classList.contains('monaco-editor') ?? false,
      ),
    ).toBe(true);
  });

  test('E2E-3 — editor does NOT steal focus while inline-renaming', async ({ page }) => {
    // Open the Add child note → Markdown submenu to enter rename mode.
    const row = page.locator('[data-testid="note-row"]').first();
    await row.click({ button: 'right' });
    await page.getByText('Add child note').hover();
    await page.getByText('Markdown').click();
    // While the rename input is alive, click another row.
    const otherRow = page.locator('[data-testid="note-row"]').nth(1);
    await otherRow.click();
    // Expect the rename input to keep focus.
    const renameInput = page.locator('[data-testid="inline-rename-input"]');
    await expect(renameInput).toBeFocused();
  });

  test('E2E-4 — rapid switch A → B → C lands focus in C', async ({ page }) => {
    const rows = page.locator('[data-testid="note-row"]');
    await Promise.all([rows.nth(0).click(), rows.nth(1).click(), rows.nth(2).click()]);
    await expect.poll(async () =>
      page.evaluate(() =>
        document.activeElement?.classList.contains('monaco-editor') ?? false,
      ),
    ).toBe(true);
    // Confirm the active tab is the third note (deepest in our test data).
    const targetId = await rows.nth(2).getAttribute('data-note-id');
    await expect.poll(async () =>
      page.evaluate(() =>
        document
          .querySelector('[data-monaco-host]')
          ?.getAttribute('data-monaco-host'),
      ),
    ).toBe(targetId);
  });

  test('E2E-5 — image-note viewer focusable; arrow keys scroll', async ({ page }) => {
    const imgRow = page
      .locator('[data-testid="note-row"][data-note-kind="image"]')
      .first();
    await imgRow.click();
    const viewer = page.locator('[data-testid="image-note-view"]');
    await expect(viewer).toBeFocused();
    const scrollBefore = await viewer.evaluate((el) => el.scrollTop);
    await page.keyboard.press('PageDown');
    const scrollAfter = await viewer.evaluate((el) => el.scrollTop);
    expect(scrollAfter).toBeGreaterThan(scrollBefore);
  });

  test('E2E-6 — mouse selection inside Monaco still works', async ({ page }) => {
    const row = page.locator('[data-testid="note-row"]').first();
    await row.click();
    const editor = page.locator('.monaco-editor').first();
    const box = await editor.boundingBox();
    if (!box) throw new Error('Monaco editor not laid out');
    await page.mouse.move(box.x + 20, box.y + 20);
    await page.mouse.down();
    await page.mouse.move(box.x + 100, box.y + 20);
    await page.mouse.up();
    // Selection length > 0.
    const len = await page.evaluate(
      () => window.getSelection()?.toString().length ?? 0,
    );
    expect(len).toBeGreaterThan(0);
  });

  test('Phase-7 — search-result click also focuses editor', async ({ page }) => {
    // Open the explorer search, type a term, click the first result.
    await page.locator('[data-testid="explorer-search-input"]').fill('Welcome');
    await page.locator('[data-testid="explorer-search-result"]').first().click();
    await expect.poll(async () =>
      page.evaluate(() =>
        document.activeElement?.classList.contains('monaco-editor') ?? false,
      ),
    ).toBe(true);
  });
});
