import { test, expect } from '@playwright/test';

test('app loads and document title contains operon-dioxus', async ({ page }) => {
  await page.goto('/');
  await expect(page).toHaveTitle(/operon-dioxus/i);
});
