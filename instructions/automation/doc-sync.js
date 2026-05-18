#!/usr/bin/env node

/**
 * doc-sync.js — Repository scanner and documentation synchronizer
 *
 * Scans the Operon repository for structural changes and updates
 * documentation files incrementally. Preserves manual edits.
 *
 * Usage:
 *   node instructions/automation/doc-sync.js [--validate] [--dry-run]
 *
 * Options:
 *   --validate   Check for broken links and outdated references (exit 1 if issues found)
 *   --dry-run    Show what would be updated without writing files
 */

const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const ROOT = path.resolve(__dirname, '..', '..');
const INSTRUCTIONS = path.resolve(__dirname, '..');

// --- Helpers ---

function readFile(filePath) {
  try {
    return fs.readFileSync(filePath, 'utf-8');
  } catch {
    return null;
  }
}

function writeFile(filePath, content) {
  fs.writeFileSync(filePath, content, 'utf-8');
}

function listDir(dirPath, opts = {}) {
  try {
    const entries = fs.readdirSync(dirPath, { withFileTypes: true });
    return entries
      .filter(e => opts.dirs ? e.isDirectory() : opts.files ? e.isFile() : true)
      .map(e => e.name);
  } catch {
    return [];
  }
}

function timestamp() {
  return new Date().toISOString().split('T')[0];
}

// --- Scanners ---

function scanCrates() {
  const cratesDir = path.join(ROOT, 'crates');
  return listDir(cratesDir, { dirs: true }).sort();
}

function scanSrcModules() {
  const srcDir = path.join(ROOT, 'src');
  return listDir(srcDir, { dirs: true }).sort();
}

function scanTestFiles() {
  const testsDir = path.join(ROOT, 'tests');
  return listDir(testsDir, { files: true })
    .filter(f => f.endsWith('.rs'))
    .sort();
}

function scanE2ESpecs() {
  const specsDir = path.join(ROOT, 'e2e', 'specs');
  return listDir(specsDir, { files: true })
    .filter(f => f.endsWith('.spec.ts'))
    .sort();
}

function scanMigrations() {
  const migrationsFile = path.join(ROOT, 'crates', 'operon-store', 'src', 'migrations.rs');
  const content = readFile(migrationsFile);
  if (!content) return [];
  const matches = content.match(/MIGRATION_\d+_\w+/g) || [];
  return matches;
}

function scanApiRoutes() {
  const routesDir = path.join(ROOT, 'crates', 'operon-api-server', 'src', 'routes');
  return listDir(routesDir, { files: true })
    .filter(f => f.endsWith('.rs'))
    .map(f => f.replace('.rs', ''))
    .sort();
}

function scanEnvVars() {
  const envVars = new Set();
  const files = [
    path.join(ROOT, 'crates', 'operon-api-server', 'src', 'main.rs'),
    path.join(ROOT, 'crates', 'operon-core', 'src', 'config.rs'),
    path.join(ROOT, 'playwright.config.ts'),
  ];
  for (const file of files) {
    const content = readFile(file);
    if (!content) continue;
    const matches = content.match(/(?:env::var|process\.env\.)\(?["']([A-Z_]+)["']\)?/g) || [];
    for (const match of matches) {
      const varName = match.match(/["']([A-Z_]+)["']/);
      if (varName) envVars.add(varName[1]);
    }
  }
  return Array.from(envVars).sort();
}

// --- Link Validator ---

function validateLinks() {
  const issues = [];
  const mdFiles = listDir(INSTRUCTIONS, { files: true }).filter(f => f.endsWith('.md'));

  for (const file of mdFiles) {
    const content = readFile(path.join(INSTRUCTIONS, file));
    if (!content) continue;

    // Find markdown links [text](target)
    const linkRegex = /\[([^\]]+)\]\(([^)]+)\)/g;
    let match;
    while ((match = linkRegex.exec(content)) !== null) {
      const target = match[2];
      // Skip external URLs and anchors
      if (target.startsWith('http') || target.startsWith('#')) continue;

      const targetPath = path.resolve(INSTRUCTIONS, target.split('#')[0]);
      if (!fs.existsSync(targetPath)) {
        issues.push({ file, link: target, type: 'broken' });
      }
    }
  }
  return issues;
}

// --- Main ---

function main() {
  const args = process.argv.slice(2);
  const validate = args.includes('--validate');
  const dryRun = args.includes('--dry-run');

  console.log(`[doc-sync] Scanning repository... (${timestamp()})`);

  const crates = scanCrates();
  const srcModules = scanSrcModules();
  const testFiles = scanTestFiles();
  const e2eSpecs = scanE2ESpecs();
  const migrations = scanMigrations();
  const apiRoutes = scanApiRoutes();
  const envVars = scanEnvVars();

  console.log(`  Crates: ${crates.length}`);
  console.log(`  Src modules: ${srcModules.length}`);
  console.log(`  Test files: ${testFiles.length}`);
  console.log(`  E2E specs: ${e2eSpecs.length}`);
  console.log(`  Migrations: ${migrations.length}`);
  console.log(`  API routes: ${apiRoutes.length}`);
  console.log(`  Env vars: ${envVars.length}`);

  // Update documentation-map.json
  const docMap = {
    _generated: timestamp(),
    crates: {},
    src_modules: srcModules,
    test_files: testFiles,
    e2e_specs: e2eSpecs,
    migrations: migrations,
    api_routes: apiRoutes,
    env_vars: envVars,
  };

  for (const crate of crates) {
    docMap.crates[crate] = [
      'architecture.md',
      'folder-structure.md',
      crate.includes('api') ? 'api-reference.md' : null,
      crate.includes('store') ? 'database-schema.md' : null,
      crate.includes('auth') ? 'security-guidelines.md' : null,
      crate.includes('plugin') ? 'how-it-works.md' : null,
    ].filter(Boolean);
  }

  if (!dryRun) {
    writeFile(
      path.join(__dirname, 'documentation-map.json'),
      JSON.stringify(docMap, null, 2) + '\n'
    );
    console.log('  Updated: documentation-map.json');
  }

  // Update index.md timestamp
  const indexPath = path.join(INSTRUCTIONS, 'index.md');
  let indexContent = readFile(indexPath);
  if (indexContent) {
    indexContent = indexContent.replace(
      /\*Last updated: \d{4}-\d{2}-\d{2}\*/,
      `*Last updated: ${timestamp()}*`
    );
    if (!dryRun) {
      writeFile(indexPath, indexContent);
      console.log('  Updated: index.md timestamp');
    }
  }

  // Validate links
  if (validate) {
    const issues = validateLinks();
    if (issues.length > 0) {
      console.error('\n[doc-sync] Broken links found:');
      for (const issue of issues) {
        console.error(`  ${issue.file} → ${issue.link} (${issue.type})`);
      }
      process.exit(1);
    } else {
      console.log('\n[doc-sync] All links valid.');
    }
  }

  console.log('[doc-sync] Done.');
}

main();
