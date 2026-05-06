// Plans-Phase-4 + Plans-Phase-8 explorer undo specs.
//
// Covers TestCase-Phase-4 E2E-1..10. Marked test.skip until the wasm
// Store harness lands (matches note-create.spec.ts pattern). Cmd+Z is
// scoped to the explorer panel; Monaco's intrinsic undo applies when
// focus is in the editor.

import { test, expect } from '@playwright/test';

test.describe.skip('Phase 4 + 8 — explorer undo', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/?test=1');
  });

  // Helper: focus the explorer panel root so Cmd+Z routes to undo.
  async function focusExplorer(page: any) {
    await page.locator('[data-explorer-root="true"]').click();
  }

  test('E2E-1 — Rename → Cmd+Z reverts title', async ({ page }) => {
    const row = page.locator('[data-testid="note-row"]').first();
    const original = await row.locator('[data-testid="note-row-name"]').innerText();
    await row.click({ button: 'right' });
    await page.getByText('Rename').click();
    const input = page.locator('[data-testid="inline-rename-input"]');
    await input.fill('Renamed');
    await input.press('Enter');
    await expect(row.locator('[data-testid="note-row-name"]')).toHaveText('Renamed');
    await focusExplorer(page);
    await page.keyboard.press('ControlOrMeta+KeyZ');
    await expect(row.locator('[data-testid="note-row-name"]')).toHaveText(original);
  });

  test('E2E-2 — Paste → Cmd+Z removes pasted subtree', async ({ page }) => {
    const a = page.locator('[data-testid="note-row"]').nth(0);
    const b = page.locator('[data-testid="note-row"]').nth(1);
    await a.click({ button: 'right' });
    await page.getByText('Copy').click();
    await b.click({ button: 'right' });
    await page.getByText('Paste').click();
    const beforeCount = await page.locator('[data-testid="note-row"]').count();
    await focusExplorer(page);
    await page.keyboard.press('ControlOrMeta+KeyZ');
    const afterCount = await page.locator('[data-testid="note-row"]').count();
    expect(afterCount).toBeLessThan(beforeCount);
  });

  test('E2E-3 — Cut+paste → Cmd+Z returns to source', async ({ page }) => {
    const source = page.locator('[data-testid="note-row"]').first();
    const sourceId = await source.getAttribute('data-note-id');
    const sourceProject = await source.evaluate((el) =>
      el.closest('[data-testid="project-row"]')?.getAttribute('data-project-id'),
    );
    await source.click({ button: 'right' });
    await page.getByText('Cut').click();
    const otherProject = page.locator('[data-testid="project-row"]').nth(1);
    await otherProject.click({ button: 'right' });
    await page.getByText('Paste').click();
    await focusExplorer(page);
    await page.keyboard.press('ControlOrMeta+KeyZ');
    // Source row back in original project.
    const restored = page.locator(`[data-note-id="${sourceId}"]`);
    const restoredProject = await restored.evaluate((el) =>
      el.closest('[data-testid="project-row"]')?.getAttribute('data-project-id'),
    );
    expect(restoredProject).toBe(sourceProject);
  });

  test('E2E-4 — Drag cross-project → Cmd+Z', async ({ page }) => {
    const source = page.locator('[data-testid="note-row"]').first();
    const sourceProject = await source.evaluate((el) =>
      el.closest('[data-testid="project-row"]')?.getAttribute('data-project-id'),
    );
    const targetProject = page.locator('[data-testid="project-row"]').nth(1);
    await source.dragTo(targetProject);
    await focusExplorer(page);
    await page.keyboard.press('ControlOrMeta+KeyZ');
    const sourceProjectAfter = await source.evaluate((el) =>
      el.closest('[data-testid="project-row"]')?.getAttribute('data-project-id'),
    );
    expect(sourceProjectAfter).toBe(sourceProject);
  });

  test('E2E-5 — Delete → Cmd+Z restores subtree', async ({ page }) => {
    const row = page.locator('[data-testid="note-row"]').first();
    const id = await row.getAttribute('data-note-id');
    await row.click({ button: 'right' });
    await page.getByText('Delete').click();
    await page.locator('[data-testid="confirm-confirm-button"]').click();
    await expect(page.locator(`[data-note-id="${id}"]`)).toHaveCount(0);
    await focusExplorer(page);
    await page.keyboard.press('ControlOrMeta+KeyZ');
    await expect(page.locator(`[data-note-id="${id}"]`)).toHaveCount(1);
  });

  test('E2E-6 — Indent → Cmd+Z outdents', async ({ page }) => {
    const row = page.locator('[data-testid="note-row"]').nth(1);
    const beforeDepth = await row.getAttribute('data-note-depth');
    await row.focus();
    await page.keyboard.press('Tab');
    const afterDepth = await row.getAttribute('data-note-depth');
    expect(Number(afterDepth)).toBeGreaterThan(Number(beforeDepth));
    await focusExplorer(page);
    await page.keyboard.press('ControlOrMeta+KeyZ');
    expect(await row.getAttribute('data-note-depth')).toBe(beforeDepth);
  });

  test('E2E-7 — Move-up → Cmd+Z restores order', async ({ page }) => {
    const rows = page.locator('[data-testid="note-row"]');
    const second = rows.nth(1);
    const firstId = await rows.nth(0).getAttribute('data-note-id');
    await second.focus();
    await page.keyboard.press('Alt+ArrowUp');
    // Now the moved row sits at position 0.
    expect(
      await rows.nth(0).getAttribute('data-note-id'),
    ).not.toBe(firstId);
    await focusExplorer(page);
    await page.keyboard.press('ControlOrMeta+KeyZ');
    expect(await rows.nth(0).getAttribute('data-note-id')).toBe(firstId);
  });

  test('E2E-8 — Cmd+Z inside Monaco does NOT pop explorer stack', async ({ page }) => {
    // Rename a note so the stack has one entry.
    const row = page.locator('[data-testid="note-row"]').first();
    const original = await row.locator('[data-testid="note-row-name"]').innerText();
    await row.click({ button: 'right' });
    await page.getByText('Rename').click();
    await page.locator('[data-testid="inline-rename-input"]').fill('Renamed');
    await page.locator('[data-testid="inline-rename-input"]').press('Enter');
    // Open the renamed note → focus moves into Monaco.
    await row.click();
    // Cmd+Z while focused in editor; Monaco intercepts.
    await page.keyboard.press('ControlOrMeta+KeyZ');
    // Title should NOT have reverted because the explorer stack wasn't
    // touched.
    await expect(row.locator('[data-testid="note-row-name"]')).toHaveText('Renamed');
    // Restore for hygiene.
    await focusExplorer(page);
    await page.keyboard.press('ControlOrMeta+KeyZ');
    await expect(row.locator('[data-testid="note-row-name"]')).toHaveText(original);
  });

  test('E2E-9 — Undo last action context-menu item', async ({ page }) => {
    const row = page.locator('[data-testid="note-row"]').first();
    const original = await row.locator('[data-testid="note-row-name"]').innerText();
    await row.click({ button: 'right' });
    await page.getByText('Rename').click();
    await page.locator('[data-testid="inline-rename-input"]').fill('X');
    await page.locator('[data-testid="inline-rename-input"]').press('Enter');
    await row.click({ button: 'right' });
    await page.getByText('Undo last action').click();
    await expect(row.locator('[data-testid="note-row-name"]')).toHaveText(original);
  });

  test('E2E-10 — Undo last action disabled when stack empty', async ({ page }) => {
    const row = page.locator('[data-testid="note-row"]').first();
    await row.click({ button: 'right' });
    const item = page.locator(
      '[data-testid="context-menu-item-undo-last-action"]',
    );
    await expect(item).toBeDisabled();
  });

  test('Phase-8 — Failed undo emits a toast', async ({ page }) => {
    // Hard to trigger an artificial repo failure from the e2e layer; this
    // spec is a placeholder for the unit-level path that emits the toast.
    // See operon-store / explorer history tests.
  });
});
