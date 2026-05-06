// Plans-Phase-1-note-creation-context-menu e2e specs.
//
// Covers TestCase-Phase-1 E2E-1..10 — submenu reveal, Markdown / Image
// leaves, project-row collapsed Add note, no standalone Add image note,
// rename input pre-selected, Enter / Escape behaviour, outside-click
// dismissal, no regression in the flat menu items.
//
// Marked test.skip per the existing note-create.spec.ts pattern. Goes
// live with the wasm Store + dx serve harness.

import { test, expect } from '@playwright/test';

test.describe.skip('Phase 1 — note-creation context menu', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/?test=1');
  });

  test('E2E-1 — Add child note → Markdown leaf creates child + opens rename', async ({ page }) => {
    const parent = page.locator('[data-testid="note-row"]').first();
    const beforeCount = await page.locator('[data-testid="note-row"]').count();
    await parent.click({ button: 'right' });
    await page.locator('[data-testid="context-menu-item-add-child-note-submenu"]').hover();
    await page.locator('[data-testid="context-menu-item-markdown"]').click();
    const afterCount = await page.locator('[data-testid="note-row"]').count();
    expect(afterCount).toBe(beforeCount + 1);
    const renameInput = page.locator('[data-testid="inline-rename-input"]');
    await expect(renameInput).toBeFocused();
  });

  test('E2E-2 — Add child note → Image leaf creates image-note row', async ({ page }) => {
    const parent = page.locator('[data-testid="note-row"]').first();
    await parent.click({ button: 'right' });
    await page.locator('[data-testid="context-menu-item-add-child-note-submenu"]').hover();
    // Stub the file picker (rfd) — actual stubbing requires harness
    // hooks; the assertion is that the new row carries data-note-kind.
    await page.locator('[data-testid="context-menu-item-image"]').click();
    const newest = page.locator('[data-testid="note-row"][data-note-kind="image"]').first();
    await expect(newest).toBeVisible();
  });

  test('E2E-3 — Add sibling note → Image leaf inserts at index+1', async ({ page }) => {
    const target = page.locator('[data-testid="note-row"]').nth(1);
    const targetIdx = await target.evaluate((el) => {
      const rows = Array.from(document.querySelectorAll('[data-testid="note-row"]'));
      return rows.indexOf(el);
    });
    await target.click({ button: 'right' });
    await page.locator('[data-testid="context-menu-item-add-sibling-note-submenu"]').hover();
    await page.locator('[data-testid="context-menu-item-image"]').click();
    // Newly minted image-note sits at targetIdx + 1.
    const inserted = page.locator('[data-testid="note-row"]').nth(targetIdx + 1);
    await expect(inserted).toHaveAttribute('data-note-kind', 'image');
  });

  test('E2E-4 — Project-row Add note → Markdown leaf creates root in rename mode', async ({ page }) => {
    const project = page.locator('[data-testid="project-row"]').first();
    await project.click({ button: 'right' });
    await page.locator('[data-testid="context-menu-item-add-note-submenu"]').hover();
    await page.locator('[data-testid="context-menu-item-markdown"]').click();
    const renameInput = page.locator('[data-testid="inline-rename-input"]');
    await expect(renameInput).toBeFocused();
  });

  test('E2E-5 — Project-row context menu does NOT contain Add image note item', async ({ page }) => {
    const project = page.locator('[data-testid="project-row"]').first();
    await project.click({ button: 'right' });
    // The standalone item used to be `Add image note…` — it's gone.
    await expect(
      page.locator('[data-testid="context-menu-item-add-image-note…"]'),
    ).toHaveCount(0);
  });

  test('E2E-6 — Newly-created note rename input has placeholder pre-selected', async ({ page }) => {
    const parent = page.locator('[data-testid="note-row"]').first();
    await parent.click({ button: 'right' });
    await page.locator('[data-testid="context-menu-item-add-child-note-submenu"]').hover();
    await page.locator('[data-testid="context-menu-item-markdown"]').click();
    const input = page.locator('[data-testid="inline-rename-input"]');
    // Type a single char immediately — selection should be replaced.
    await input.press('H');
    await expect(input).toHaveValue('H');
  });

  test('E2E-7 — Enter commits the new title', async ({ page }) => {
    const parent = page.locator('[data-testid="note-row"]').first();
    await parent.click({ button: 'right' });
    await page.locator('[data-testid="context-menu-item-add-child-note-submenu"]').hover();
    await page.locator('[data-testid="context-menu-item-markdown"]').click();
    const input = page.locator('[data-testid="inline-rename-input"]');
    await input.fill('Hello');
    await input.press('Enter');
    await expect(
      page.locator('[data-testid="note-row-name"]', { hasText: 'Hello' }),
    ).toBeVisible();
  });

  test('E2E-8 — Escape on freshly-created note removes the placeholder row', async ({ page }) => {
    const parent = page.locator('[data-testid="note-row"]').first();
    const beforeCount = await page.locator('[data-testid="note-row"]').count();
    await parent.click({ button: 'right' });
    await page.locator('[data-testid="context-menu-item-add-child-note-submenu"]').hover();
    await page.locator('[data-testid="context-menu-item-markdown"]').click();
    await page.locator('[data-testid="inline-rename-input"]').press('Escape');
    const afterCount = await page.locator('[data-testid="note-row"]').count();
    expect(afterCount).toBe(beforeCount);
  });

  test('E2E-9 — Click outside the menu closes the whole chain', async ({ page }) => {
    const parent = page.locator('[data-testid="note-row"]').first();
    await parent.click({ button: 'right' });
    await page.locator('[data-testid="context-menu-item-add-child-note-submenu"]').hover();
    await expect(page.locator('[data-testid="context-submenu"]')).toHaveCount(1);
    // Click the scrim (covers the rest of the viewport).
    await page.locator('[data-testid="context-menu-scrim"]').click({ position: { x: 1, y: 1 } });
    await expect(page.locator('[data-testid="context-menu"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="context-submenu"]')).toHaveCount(0);
  });

  test('E2E-10 — Existing flat items still work (smoke)', async ({ page }) => {
    const row = page.locator('[data-testid="note-row"]').first();
    await row.click({ button: 'right' });
    await page.locator('[data-testid="context-menu-item-rename"]').click();
    await expect(page.locator('[data-testid="inline-rename-input"]')).toBeFocused();
  });

  // Bug-c236b2ed (Notes 2 / Add Note submenu): hovering one submenu
  // trigger then a sibling submenu trigger used to leave both submenus
  // visible at once, anchored at different vertical offsets — what the
  // user described as "appears twice or far away from the context
  // menu". Mutual exclusion now lives in the parent ContextMenu's
  // open_submenu signal.
  test('E2E-11 — Mutual exclusion: only one submenu open at a time', async ({ page }) => {
    const row = page.locator('[data-testid="note-row"]').first();
    await row.click({ button: 'right' });
    // Open "Add child note" submenu.
    await page.locator('[data-testid="context-menu-item-add-child-note-submenu"]').hover();
    await expect(page.locator('[data-testid="context-submenu"]')).toHaveCount(1);
    // Hover the sibling "Add sibling note" trigger; the previous submenu
    // must close so total submenu count remains 1.
    await page.locator('[data-testid="context-menu-item-add-sibling-note-submenu"]').hover();
    await expect(page.locator('[data-testid="context-submenu"]')).toHaveCount(1);
    // Hover a non-submenu sibling (Rename) — the submenu must close
    // entirely.
    await page.locator('[data-testid="context-menu-item-rename"]').hover();
    await expect(page.locator('[data-testid="context-submenu"]')).toHaveCount(0);
  });
});
