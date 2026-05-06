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
});
