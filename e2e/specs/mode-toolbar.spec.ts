// Phase 2 e2e: ModeToolbar capability honesty.
//
// Asserts the per-tab toolbar exposes only the modes the active plugin's
// capabilities() claims. Markdown/Plaintext/JSON ship VIEW | EDIT this
// phase — Live Preview button is hidden (markdown LIVE_PREVIEW bit flips
// on in Phase 4 once CodeMirror 6 lands).
//
// These specs run against `dx serve --platform web`; the Justfile
// `test-e2e` recipe depends on `build-bridge` so the editor bridge dist
// is present before Playwright fires.

import { test, expect } from '@playwright/test';

test.describe('ModeToolbar', () => {
  test('renders View and Edit buttons for an open markdown tab', async ({ page }) => {
    await page.goto('/');

    // Open the first markdown sample from the Notes Explorer side bar.
    const firstNote = page.locator('.notes-explorer-row').first();
    await firstNote.waitFor({ state: 'visible', timeout: 30_000 });
    await firstNote.click();

    const toolbar = page.locator('[data-component="mode-toolbar"]');
    await expect(toolbar).toBeVisible();

    // Markdown plugin claims VIEW | EDIT this phase. Split is a derived
    // mode (only when both VIEW + EDIT) so the Split button also appears.
    await expect(toolbar.locator('[data-mode="view"]')).toBeVisible();
    await expect(toolbar.locator('[data-mode="edit"]')).toBeVisible();
    await expect(toolbar.locator('[data-mode="split"]')).toBeVisible();

    // Live Preview must NOT appear until Phase 4 lands CM6 + the
    // markdown LIVE_PREVIEW capability flag flips.
    await expect(toolbar.locator('[data-mode="live-preview"]')).toHaveCount(0);
  });

  test('clicking Edit sets data-active-mode="edit"', async ({ page }) => {
    await page.goto('/');

    const firstNote = page.locator('.notes-explorer-row').first();
    await firstNote.waitFor({ state: 'visible', timeout: 30_000 });
    await firstNote.click();

    const toolbar = page.locator('[data-component="mode-toolbar"]');
    await expect(toolbar).toHaveAttribute('data-active-mode', 'view');

    await toolbar.locator('[data-mode="edit"]').click();
    await expect(toolbar).toHaveAttribute('data-active-mode', 'edit');

    // Active button has aria-pressed="true" — accessibility contract.
    const editBtn = toolbar.locator('[data-mode="edit"]');
    await expect(editBtn).toHaveAttribute('aria-pressed', 'true');
    const viewBtn = toolbar.locator('[data-mode="view"]');
    await expect(viewBtn).toHaveAttribute('aria-pressed', 'false');
  });

  test('Edit mode mounts the Monaco host element', async ({ page }) => {
    await page.goto('/');
    await page.locator('.notes-explorer-row').first().click();

    const toolbar = page.locator('[data-component="mode-toolbar"]');
    await toolbar.locator('[data-mode="edit"]').click();

    // Host is rendered immediately; the actual Monaco instance mounts
    // asynchronously. The host element existing is the Phase-2
    // acceptance bar; Phase 1's wasm-bindgen-test already verifies the
    // mount itself.
    const host = page.locator('.operon-monaco-host');
    await expect(host).toBeVisible();
    await expect(host).toHaveAttribute('data-monaco-language', 'markdown');
  });

  test('mode toolbar does not appear when no tab is open', async ({ page }) => {
    await page.goto('/');
    // Empty toolbar variant has the operon-mode-toolbar-empty marker —
    // still in the DOM (so layout doesn't shift when a tab opens) but
    // no buttons inside.
    const toolbar = page.locator('.operon-mode-toolbar');
    await expect(toolbar).toBeVisible();
    await expect(toolbar.locator('button')).toHaveCount(0);
  });
});
