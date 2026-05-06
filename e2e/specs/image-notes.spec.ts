// Plans-Phase-6-image-notes e2e specs.
//
// Covers Add image note via picker, in-editor insert (Cmd/Ctrl+Shift+I),
// paste-image (clipboard image), explorer file-drop, image-tab view,
// blob GC on delete, [md]/[im] indicator.
//
// Skipped until the Phase 2 wasm Store lands.

import { test, expect } from '@playwright/test';

test.describe.skip('Phase 6 — image notes', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/?test=1');
  });

  test('Add image note via project context menu', async ({ page }) => {
    const project = page.locator('[data-testid="project-row"]').first();
    await project.click({ button: 'right' });
    await page.getByText('Add image note…').click();
    // OS file picker fires; on supported targets we'd inject a fixture.
  });

  test('Cmd/Ctrl+Shift+I in markdown editor inserts ![[…]] embed', async ({ page }) => {
    const editor = page.locator('[data-testid="local-note-textarea"]');
    await editor.focus();
    await page.keyboard.press('ControlOrMeta+Shift+KeyI');
    // OS picker fires.
  });

  test('Paste an image into the markdown body creates a child image-note', async ({ page }) => {
    const editor = page.locator('[data-testid="local-note-textarea"]');
    await editor.focus();
    // Inject image bytes into the clipboard via DataTransfer.
    // Embed expected: `![[<title>^<short-id>]]` appears at caret.
  });

  test('Drop an image file onto a note row creates a child image-note', async ({ page }) => {
    const row = page.locator('[data-testid="note-row"]').first();
    // page.locator(...).dispatchEvent('drop', {dataTransfer:...})
  });

  // Bug-c236b2ed (Notes 2 / Image note): the user listed "drop an image
  // to the note area" as one of the three image-note ingestion paths.
  // The textarea now has ondragover/ondrop wired so a file dropped onto
  // the editor body is written, child image-note minted, and `![[…]]`
  // spliced at the caret — same end-state as Cmd/Ctrl+Shift+I.
  test('Drop an image file onto the editor body creates a child image-note + splices ![[…]]', async ({ page }) => {
    const editor = page.locator('[data-testid="local-note-textarea"]');
    await editor.focus();
    // Harness hook: dispatch a synthetic `drop` carrying a tiny PNG.
    // Expected:
    //   1. New row with data-note-kind="image" appears in the explorer.
    //   2. Body contains `![[<stem>^<short>]]` at the caret.
    //   3. Tab is dirty (manual-save indicator visible).
  });

  test('Explorer rows show [md]/[im] indicator', async ({ page }) => {
    const md = page.locator('[data-testid="kind-badge"][data-note-kind="markdown"]').first();
    const im = page.locator('[data-testid="kind-badge"][data-note-kind="image"]').first();
    await expect(md).toContainText('[md]');
    await expect(im).toContainText('[im]');
  });

  test('Open an image note → image-tab view renders an <img>', async ({ page }) => {
    const im = page.locator('[data-testid="note-row"][data-note-kind="image"]').first();
    await im.click();
    const view = page.locator('[data-testid="image-note-view"] img');
    await expect(view).toBeVisible();
  });

  test('Deleting the only referrer of a blob also removes the on-disk file', async ({ page }) => {
    // 1. Create image note A; verify blob exists at <vault>/.operon/images/<sha>.png.
    // 2. Delete A.
    // 3. Verify blob file is gone.
  });
});
