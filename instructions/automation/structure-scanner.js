#!/usr/bin/env node

/**
 * structure-scanner.js — Project structure scanner
 *
 * Scans folders, routes, services, components, schemas, and config files.
 * Updates documentation-map.json with current project structure.
 *
 * Usage:
 *   node instructions/automation/structure-scanner.js
 */

const fs = require('fs');
const path = require('path');

const ROOT = path.resolve(__dirname, '..', '..');
const MAP_PATH = path.join(__dirname, 'documentation-map.json');

// --- Helpers ---

function listDir(dirPath, opts = {}) {
  try {
    const entries = fs.readdirSync(dirPath, { withFileTypes: true });
    return entries
      .filter(e => {
        if (opts.dirs) return e.isDirectory();
        if (opts.files) return e.isFile();
        return true;
      })
      .map(e => e.name);
  } catch {
    return [];
  }
}

function walkDir(dirPath, maxDepth = 3, currentDepth = 0) {
  if (currentDepth >= maxDepth) return [];
  const results = [];
  const entries = listDir(dirPath);

  for (const entry of entries) {
    const fullPath = path.join(dirPath, entry);
    const relativePath = path.relative(ROOT, fullPath);

    // Skip hidden dirs, node_modules, target
    if (entry.startsWith('.') || entry === 'node_modules' || entry === 'target') continue;

    try {
      const stat = fs.statSync(fullPath);
      if (stat.isDirectory()) {
        results.push({ path: relativePath + '/', type: 'directory' });
        results.push(...walkDir(fullPath, maxDepth, currentDepth + 1));
      } else if (stat.isFile()) {
        results.push({ path: relativePath, type: 'file' });
      }
    } catch {
      // Skip inaccessible paths
    }
  }

  return results;
}

// --- Scanners ---

function scanCrates() {
  const cratesDir = path.join(ROOT, 'crates');
  const crates = listDir(cratesDir, { dirs: true });
  const result = {};

  for (const crate of crates) {
    const cargoToml = path.join(cratesDir, crate, 'Cargo.toml');
    const srcDir = path.join(cratesDir, crate, 'src');

    result[crate] = {
      has_cargo_toml: fs.existsSync(cargoToml),
      source_files: listDir(srcDir, { files: true }).filter(f => f.endsWith('.rs')),
      source_dirs: listDir(srcDir, { dirs: true }),
    };
  }

  return result;
}

function scanSrcModules() {
  const srcDir = path.join(ROOT, 'src');
  const modules = listDir(srcDir, { dirs: true });
  const result = {};

  for (const mod of modules) {
    const modDir = path.join(srcDir, mod);
    result[mod] = {
      files: listDir(modDir, { files: true }).filter(f => f.endsWith('.rs')),
      subdirs: listDir(modDir, { dirs: true }),
    };
  }

  return result;
}

function scanConfigFiles() {
  const configs = [
    'Cargo.toml', 'Dioxus.toml', 'Justfile', 'package.json',
    'playwright.config.ts', 'rust-toolchain.toml', 'clippy.toml',
    'deny.toml', 'tailwind.css', 'index.html', 'tsconfig.json',
  ];

  return configs.filter(f => fs.existsSync(path.join(ROOT, f)));
}

function scanAssets() {
  const assetsDir = path.join(ROOT, 'assets');
  return {
    css: listDir(assetsDir, { files: true }).filter(f => f.endsWith('.css')),
    editor_bridge: listDir(path.join(assetsDir, 'editor-bridge'), { files: true })
      .filter(f => f.endsWith('.ts') || f.endsWith('.json')),
  };
}

function scanTests() {
  return {
    integration: listDir(path.join(ROOT, 'tests'), { files: true }).filter(f => f.endsWith('.rs')),
    wasm: listDir(path.join(ROOT, 'tests-wasm', 'src'), { files: true }).filter(f => f.endsWith('.rs')),
    e2e_specs: listDir(path.join(ROOT, 'e2e', 'specs'), { files: true }).filter(f => f.endsWith('.ts')),
    e2e_pages: listDir(path.join(ROOT, 'e2e', 'pages'), { files: true }).filter(f => f.endsWith('.ts')),
  };
}

function scanSeedSkills() {
  const dirs = ['seed-skills', 'seed-skills-employee', 'seed-skills-sum', 'seed-skills-updated'];
  const result = {};

  for (const dir of dirs) {
    const dirPath = path.join(ROOT, dir);
    if (fs.existsSync(dirPath)) {
      result[dir] = listDir(dirPath, { files: true }).filter(f => f.endsWith('.md'));
    }
  }

  return result;
}

// --- Documentation Mapping ---

function buildDocMap(crates) {
  const docMapping = {};

  for (const crate of Object.keys(crates)) {
    const docs = ['architecture.md', 'folder-structure.md'];

    if (crate.includes('api-server')) docs.push('api-reference.md', 'deployment-guide.md');
    if (crate.includes('store')) docs.push('database-schema.md');
    if (crate.includes('auth')) docs.push('security-guidelines.md');
    if (crate.includes('core')) docs.push('how-it-works.md', 'coding-guidelines.md');
    if (crate.includes('plugin')) docs.push('how-it-works.md', 'tech-stack.md');
    if (crate.includes('export')) docs.push('how-it-works.md');
    if (crate.includes('notes')) docs.push('how-it-works.md');
    if (crate.includes('agent-cli')) docs.push('setup-guide.md', 'build-guide.md');

    docMapping[`crates/${crate}`] = docs;
  }

  // Source modules
  docMapping['src/agent'] = ['architecture.md', 'how-it-works.md'];
  docMapping['src/commands'] = ['how-it-works.md', 'folder-structure.md'];
  docMapping['src/editor'] = ['architecture.md', 'how-it-works.md'];
  docMapping['src/local_mode'] = ['how-it-works.md', 'folder-structure.md'];
  docMapping['src/persistence'] = ['architecture.md', 'how-it-works.md'];
  docMapping['src/plugin'] = ['architecture.md', 'how-it-works.md'];
  docMapping['src/shell'] = ['architecture.md', 'folder-structure.md'];
  docMapping['src/tabs'] = ['how-it-works.md'];
  docMapping['src/theme'] = ['how-it-works.md'];

  return docMapping;
}

// --- Main ---

function main() {
  console.log('[structure-scanner] Scanning project structure...');

  const crates = scanCrates();
  const srcModules = scanSrcModules();
  const configs = scanConfigFiles();
  const assets = scanAssets();
  const tests = scanTests();
  const seedSkills = scanSeedSkills();
  const docMapping = buildDocMap(crates);

  const structure = {
    _generated: new Date().toISOString().split('T')[0],
    _tool: 'structure-scanner.js',
    crates,
    src_modules: srcModules,
    config_files: configs,
    assets,
    tests,
    seed_skills: seedSkills,
    documentation_mapping: docMapping,
  };

  fs.writeFileSync(MAP_PATH, JSON.stringify(structure, null, 2) + '\n', 'utf-8');
  console.log(`  Crates: ${Object.keys(crates).length}`);
  console.log(`  Source modules: ${Object.keys(srcModules).length}`);
  console.log(`  Config files: ${configs.length}`);
  console.log(`  Test files: ${tests.integration.length + tests.e2e_specs.length}`);
  console.log(`  Updated: documentation-map.json`);
}

main();
