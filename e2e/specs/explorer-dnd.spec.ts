// Plans-Phase-3-explorer-drag-drop-feedback e2e specs (TestCase-Phase-3
// E2E-1..8 plus Phase-7 snap + hover-to-expand assertions).
//
// HTML5 drag-and-drop is fiddly under Playwright; we use
// `page.dispatchEvent(...)` to fire dragstart/dragover/drop manually
// when the built-in `dragAndDrop` doesn't carry our DragSession across
// the JS-side payload. Marked `test.skip` until the wasm Store + dx
// serve harness is wired (matching note-create.spec.ts).

import { test, expect } from '@playwright/test';

test.describe.skip('Phase 3 + 7 — explorer drag-and-drop feedback', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/?test=1');
  });

  test('E2E-1 — drag note onto sibling After zone shows indicator + commits', async ({ page }) => {
    const a = page.locator('[data-testid="note-row"]').nth(0);
    const b = page.locator('[data-testid="note-row"]').nth(1);
    await a.dragTo(b, { targetPosition: { x: 50, y: 22 } });
    // After commit, [B, A, C] order — assert by data-note-id sequence.
    const ids = await page
      .locator('[data-testid="note-row"]')
      .evaluateAll((els) => els.map((e) => e.getAttribute('data-note-id')));
    const aId = await a.getAttribute('data-note-id');
    const bId = await b.getAttribute('data-note-id');
    expect(ids.indexOf(bId!)).toBeLessThan(ids.indexOf(aId!));
  });

  test('E2E-2 — drag note onto a different project row (Into) commits', async ({ page }) => {
    const noteInP1 = page.locator('[data-testid="note-row"]').first();
    const project2Row = page
      .locator('[data-testid="project-row"]')
      .nth(1);
    await noteInP1.dragTo(project2Row);
    // Assert noteInP1's data-note-id now appears under project2.
    // (Selector elaborated when test data fixture is finalised.)
    await expect(project2Row).toHaveAttribute('data-open', 'true');
  });

  test('E2E-3 — cross-project drop preserves subtree', async ({ page }) => {
    // Pre-seed: project1 has A→B→C subtree; project2 has D.
    const a = page.locator('[data-note-id="A-uuid"]');
    const d = page.locator('[data-note-id="D-uuid"]');
    await a.dragTo(d, { targetPosition: { x: 50, y: 14 } });
    // Verify B and C still descend from A in their new home.
    const a_in_p2 = page.locator(
      '[data-testid="project-row"]:nth-child(2) [data-note-id="A-uuid"]',
    );
    await expect(a_in_p2).toBeVisible();
    const b_in_p2 = page.locator(
      '[data-testid="project-row"]:nth-child(2) [data-note-id="B-uuid"]',
    );
    await expect(b_in_p2).toBeVisible();
  });

  test('E2E-4 — snap emphasis after 80 ms hover', async ({ page }) => {
    const a = page.locator('[data-testid="note-row"]').nth(0);
    const b = page.locator('[data-testid="note-row"]').nth(1);
    // Manually fire a stable dragover stream to keep the cursor over B.
    await a.dispatchEvent('dragstart');
    for (let i = 0; i < 5; i++) {
      await b.dispatchEvent('dragover');
      await page.waitForTimeout(20);
    }
    await expect(
      page.locator('[data-testid="drop-indicator-into-snap"]'),
    ).toHaveCount(1);
  });

  test('E2E-5 — forbidden indicator on self-drop', async ({ page }) => {
    const row = page.locator('[data-testid="note-row"]').first();
    await row.dispatchEvent('dragstart');
    await row.dispatchEvent('dragover');
    await expect(
      page.locator('[data-testid="drop-indicator-forbidden"]'),
    ).toHaveCount(1);
  });

  test('E2E-6 — drop rejected on row mid-rename', async ({ page }) => {
    const target = page.locator('[data-testid="note-row"]').nth(1);
    // Trigger rename on target.
    await target.click({ button: 'right' });
    await page.getByText('Rename').click();
    // Now drag a different row over it.
    const source = page.locator('[data-testid="note-row"]').nth(0);
    await source.dragTo(target);
    // Indicator should not appear; tree state unchanged.
    await expect(
      page.locator('[data-testid="drop-indicator-into"]'),
    ).toHaveCount(0);
  });

  test('E2E-7 — project→project Before/After only', async ({ page }) => {
    const p1 = page.locator('[data-testid="project-row"]').nth(0);
    const p2 = page.locator('[data-testid="project-row"]').nth(1);
    // Top half → Before zone.
    await p1.dragTo(p2, { targetPosition: { x: 50, y: 4 } });
    await expect(
      page.locator('[data-testid="drop-indicator-before"]').first(),
    ).toBeVisible();
    // Middle 40% → forbidden for project→project.
    await p1.dispatchEvent('dragstart');
    await p2.dispatchEvent('dragover', { clientY: 14 });
    await expect(
      page.locator('[data-testid="drop-indicator-forbidden"]'),
    ).toHaveCount(1);
  });

  test('E2E-8 — Esc cancels drag cleanly', async ({ page }) => {
    const a = page.locator('[data-testid="note-row"]').nth(0);
    await a.dispatchEvent('dragstart');
    await page.keyboard.press('Escape');
    await expect(
      page.locator('[data-testid^="drop-indicator-"]'),
    ).toHaveCount(0);
  });

  test('Phase-7 — 600 ms hover-to-expand on collapsed parent', async ({ page }) => {
    const collapsed = page
      .locator('[data-testid="note-row"][data-open="false"]')
      .first();
    const dragSrc = page.locator('[data-testid="note-row"]').last();
    await dragSrc.dispatchEvent('dragstart');
    // Hover Into zone for ≥600 ms by streaming dragover events.
    for (let i = 0; i < 12; i++) {
      await collapsed.dispatchEvent('dragover');
      await page.waitForTimeout(60);
    }
    await expect(collapsed).toHaveAttribute('data-open', 'true');
  });
});
