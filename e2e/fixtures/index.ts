// Phase 4 of the Playwright-for-testing seed extends `test` with named
// page-object fixtures. Phase 1 re-exports the upstream `test` and `expect`
// so specs can import from a stable path today and pick up the richer
// fixture set as Phase 4 lands without churn at the call sites.

export { test, expect } from '@playwright/test';
