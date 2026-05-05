// e2e coverage for the "Themes – icons – shell." Archon seed
// (e971829d-35e0-47f6-bb4c-26c82b0824de):
//   - Color Theme picker via Ctrl+Shift+P + "Color Theme..." command
//   - Cycles through every shipped theme; data-theme + data-theme-id update
//   - Selected theme persists across reload (operon.theme.id in localStorage)
//   - Escape inside the picker reverts to the originally active theme
//   - Every former-glyph spot in the shell now renders an <svg>

import { test, expect, Page } from '../fixtures';

const ALL_THEMES = [
  { slug: 'vscode-dark-plus', displayName: 'VSCode Dark+', kind: 'dark' },
  { slug: 'vscode-light-plus', displayName: 'VSCode Light+', kind: 'light' },
  { slug: 'nord', displayName: 'Nord', kind: 'dark' },
  { slug: 'monokai-pro', displayName: 'Monokai Pro', kind: 'dark' },
  { slug: 'solarized-dark', displayName: 'Solarized Dark', kind: 'dark' },
  { slug: 'solarized-light', displayName: 'Solarized Light', kind: 'light' },
  { slug: 'abyss', displayName: 'Abyss', kind: 'dark' },
  { slug: 'kimbie-dark', displayName: 'Kimbie Dark', kind: 'dark' },
  { slug: 'high-contrast-dark', displayName: 'High Contrast Dark', kind: 'hc-dark' },
] as const;

async function clearStorage(page: Page) {
  await page.evaluate(() => {
    try {
      localStorage.clear();
    } catch (_) {
      // Some private-mode contexts disallow access; ignore.
    }
  });
}

async function openCommandPalette(page: Page) {
  // The Shell registers Ctrl+Shift+P (or Meta+Shift+P) for the Commands palette.
  await page.locator('#operon-shell').focus();
  await page.keyboard.press('Control+Shift+P');
  await expect(page.locator('[data-component="command-palette"]')).toBeVisible();
}

async function pickThemeByName(page: Page, displayName: string) {
  await openCommandPalette(page);
  // Type the command title; Enter runs `workbench.action.selectTheme`.
  const input = page.locator('.operon-palette-input');
  await input.fill('Color Theme');
  await page.keyboard.press('Enter');
  // Picker now in Themes mode.
  await expect(
    page.locator('[data-component="command-palette"][data-palette-mode="themes"]'),
  ).toBeVisible();
  await page.locator('.operon-palette-input').fill(displayName);
  await page.keyboard.press('Enter');
}

test.describe('theme picker', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await clearStorage(page);
    await page.reload();
    await expect(page.locator('#operon-shell')).toBeVisible();
  });

  test('cycles through every shipped theme', async ({ page }) => {
    const root = page.locator('#operon-root');
    for (const t of ALL_THEMES) {
      await pickThemeByName(page, t.displayName);
      await expect(root).toHaveAttribute('data-theme-id', t.slug);
      await expect(root).toHaveAttribute('data-theme', t.kind);
    }
  });

  test('selected theme persists across reload', async ({ page }) => {
    const root = page.locator('#operon-root');
    await pickThemeByName(page, 'Solarized Dark');
    await expect(root).toHaveAttribute('data-theme-id', 'solarized-dark');

    await page.reload();
    await expect(page.locator('#operon-shell')).toBeVisible();
    await expect(root).toHaveAttribute('data-theme-id', 'solarized-dark');
  });

  test('Escape reverts to the originally active theme', async ({ page }) => {
    const root = page.locator('#operon-root');
    const before = await root.getAttribute('data-theme-id');
    expect(before).toBeTruthy();

    await openCommandPalette(page);
    await page.locator('.operon-palette-input').fill('Color Theme');
    await page.keyboard.press('Enter');
    // In Themes mode, type a different theme to focus it (live-preview applies).
    await page.locator('.operon-palette-input').fill('Abyss');
    await page.keyboard.press('Escape');

    await expect(page.locator('[data-component="command-palette"]')).toBeHidden();
    await expect(root).toHaveAttribute('data-theme-id', before!);
  });
});

test.describe('icons are svg, not glyphs', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('#operon-shell')).toBeVisible();
  });

  test('every prior glyph spot renders an <svg> child', async ({ page }) => {
    // Menubar right-side toggles (panel + companion).
    const toggleSvgs = page.locator('.operon-menubar-right .operon-toggle-btn svg');
    await expect(toggleSvgs).toHaveCount(2);

    // Activity bar collapse toggle.
    await expect(page.locator('.operon-activity-toggle svg')).toBeVisible();

    // Notes Explorer activity icon.
    await expect(
      page.locator('[data-activity-id="notes-explorer:default"] svg'),
    ).toBeVisible();
  });
});
