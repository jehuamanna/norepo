// Plans-Phase-5-vfs-wikilinks e2e specs.
//
// Covers parse + render, click-to-navigate, broken-link styling,
// LinkPicker (Cmd/Ctrl+K), rename propagation through referrer bodies.
//
// Skipped until the Phase 2 wasm Store lands.

import { test, expect } from '@playwright/test';

test.describe.skip('Phase 5 — VFS + Obsidian wikilinks', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/?test=1');
  });

  test('[[…]] in body renders as <a class="wikilink">', async ({ page }) => {
    // Pre-condition: a note with body `[[Other]]` is open in the active tab.
    const link = page.locator('a.wikilink').first();
    await expect(link).toBeVisible();
    await expect(link).toHaveAttribute('data-wikilink-target', /.+/);
  });

  test('Clicking a wikilink opens the linked note in a tab', async ({ page }) => {
    const link = page.locator('a.wikilink').first();
    await link.click();
    // Active tab now shows the linked note.
  });

  test('Cmd/Ctrl+K opens the LinkPicker; pick inserts [[…]] at caret', async ({ page }) => {
    const editor = page.locator('[data-testid="local-note-textarea"]');
    await editor.focus();
    await page.keyboard.press('ControlOrMeta+KeyK');
    const picker = page.locator('[data-testid="link-picker"]');
    await expect(picker).toBeVisible();
    await picker.locator('[data-testid="link-picker-query"]').fill('Other');
    const first = picker.locator('[data-testid="link-picker-result"]').first();
    await first.click();
    await expect(editor).toContainText(/\[\[.+\]\]/);
  });

  test('Broken wikilink renders with wikilink-broken class', async ({ page }) => {
    // Pre-condition: a note's body references a since-deleted target.
    const broken = page.locator('a.wikilink.wikilink-broken').first();
    await expect(broken).toBeVisible();
    await expect(broken).toHaveAttribute('data-wikilink-broken', 'true');
  });

  test('Renaming a target rewrites every referrer body', async ({ page }) => {
    // 1. Note "Source" body contains `[[Target]]`.
    // 2. Rename Target → Aimed.
    // 3. Open Source — body now contains `[[Aimed]]`.
  });

  // Plans-Phase-9-wikilink-picker (rev 1): kind-aware picker so picking
  // an image note inserts `![[…]]` (renders inline as <img>) instead of
  // `[[…]]` (text anchor). Picker rows surface a `[md]` / `[im]` badge
  // so the user can tell them apart before picking.
  test('LinkPicker shows [md] / [im] kind badge per result row', async ({ page }) => {
    const editor = page.locator('[data-testid="local-note-textarea"]');
    await editor.focus();
    await page.keyboard.press('ControlOrMeta+KeyK');
    await page
      .locator('[data-testid="link-picker"] [data-testid="link-picker-query"]')
      .fill('Pic');
    const imageRow = page
      .locator('[data-testid="link-picker-result"][data-note-kind="image"]')
      .first();
    await expect(imageRow.locator('[data-testid="kind-badge"]')).toContainText('[im]');
  });

  test('Picking an image note inserts ![[…]] (embed) — renders inline in View mode', async ({ page }) => {
    const editor = page.locator('[data-testid="local-note-textarea"]');
    await editor.focus();
    await page.keyboard.press('ControlOrMeta+KeyK');
    await page
      .locator('[data-testid="link-picker"] [data-testid="link-picker-query"]')
      .fill('Pic');
    await page
      .locator('[data-testid="link-picker-result"][data-note-kind="image"]')
      .first()
      .click();
    await expect(editor).toContainText(/!\[\[.+\]\]/);
    // Switch the tab to View mode and confirm the embed resolves to <img>.
    await page.locator('[data-testid="mode-toolbar-view"]').click();
    await expect(page.locator('[data-wikilink-embed="image"]')).toBeVisible();
  });

  test('Picking a markdown note still inserts [[…]] (text anchor)', async ({ page }) => {
    const editor = page.locator('[data-testid="local-note-textarea"]');
    await editor.focus();
    await page.keyboard.press('ControlOrMeta+KeyK');
    await page
      .locator('[data-testid="link-picker"] [data-testid="link-picker-query"]')
      .fill('Other');
    await page
      .locator('[data-testid="link-picker-result"][data-note-kind="markdown"]')
      .first()
      .click();
    // Markdown picks must NOT carry the leading `!` — distinguishes
    // them from image embeds asserted above.
    await expect(editor).not.toContainText(/!\[\[.+\]\]/);
    await expect(editor).toContainText(/\[\[.+\]\]/);
  });

  test('Right-click on the editor body opens an "Insert reference…" menu that triggers the LinkPicker', async ({ page }) => {
    const editor = page.locator('[data-testid="local-note-textarea"]');
    await editor.click({ button: 'right' });
    const menuItem = page.locator(
      '[data-testid="context-menu-item-insert-reference…"]',
    );
    await expect(menuItem).toBeVisible();
    await menuItem.click();
    await expect(page.locator('[data-testid="link-picker"]')).toBeVisible();
  });
});
