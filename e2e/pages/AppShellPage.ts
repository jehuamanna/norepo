// Phase 4 of the Playwright-for-testing seed fleshes out the AppShellPage
// Page Object Model. Phase 1 lands only this stub so the folder layout is
// committed and the import path stable.
//
// See Plans-Phase-4-playwright-e2e-scaffolding (Archon) for the full
// expected surface.

import type { Page } from '@playwright/test';

export class AppShellPage {
  constructor(public readonly page: Page) {}

  async goto() {
    await this.page.goto('/');
  }
}
