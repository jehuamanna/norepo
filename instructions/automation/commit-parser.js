#!/usr/bin/env node

/**
 * commit-parser.js — Parse and categorize Git commits
 *
 * Reads git log and categorizes commits by type (feat, fix, docs, etc.)
 * Extracts affected modules and generates summaries.
 *
 * Usage:
 *   node instructions/automation/commit-parser.js [--since=<date>] [--json]
 *
 * Options:
 *   --since=YYYY-MM-DD   Only parse commits after this date
 *   --json               Output raw JSON instead of formatted text
 */

const { execSync } = require('child_process');

const ROOT = require('path').resolve(__dirname, '..', '..');

// --- Commit Categories ---

const CATEGORIES = {
  feat: 'Features',
  fix: 'Fixes',
  docs: 'Documentation',
  refactor: 'Refactors',
  test: 'Tests',
  chore: 'Chores',
  perf: 'Performance',
  ci: 'CI/CD',
  style: 'Style',
  build: 'Build',
};

// --- Git Helpers ---

function getCommits(since) {
  const sinceArg = since ? `--since="${since}"` : '';
  try {
    const log = execSync(
      `git -C "${ROOT}" log ${sinceArg} --pretty=format:"%H|%h|%s|%an|%ai" --no-merges`,
      { encoding: 'utf-8', maxBuffer: 10 * 1024 * 1024 }
    );
    return log.trim().split('\n').filter(Boolean).map(line => {
      const [hash, short, subject, author, date] = line.split('|');
      return { hash, short, subject, author, date };
    });
  } catch {
    return [];
  }
}

function getChangedFiles(hash) {
  try {
    const diff = execSync(
      `git -C "${ROOT}" diff-tree --no-commit-id --name-only -r ${hash}`,
      { encoding: 'utf-8' }
    );
    return diff.trim().split('\n').filter(Boolean);
  } catch {
    return [];
  }
}

// --- Parser ---

function parseCommit(commit) {
  const { subject } = commit;

  // Parse conventional commit: type(scope): description
  const conventionalMatch = subject.match(/^(\w+)(?:\(([^)]+)\))?:\s*(.+)$/);

  let type = 'other';
  let scope = null;
  let description = subject;

  if (conventionalMatch) {
    type = conventionalMatch[1].toLowerCase();
    scope = conventionalMatch[2] || null;
    description = conventionalMatch[3];
  } else {
    // Heuristic categorization
    const lower = subject.toLowerCase();
    if (lower.includes('fix') || lower.includes('bug')) type = 'fix';
    else if (lower.includes('add') || lower.includes('implement') || lower.includes('feature')) type = 'feat';
    else if (lower.includes('refactor') || lower.includes('clean')) type = 'refactor';
    else if (lower.includes('test')) type = 'test';
    else if (lower.includes('doc') || lower.includes('readme')) type = 'docs';
    else if (lower.includes('update') || lower.includes('upgrade') || lower.includes('bump')) type = 'chore';
  }

  // Detect affected modules from changed files
  const files = getChangedFiles(commit.hash);
  const modules = new Set();

  for (const file of files) {
    if (file.startsWith('crates/')) {
      const crate = file.split('/')[1];
      modules.add(crate);
    } else if (file.startsWith('src/')) {
      const mod = file.split('/')[1];
      modules.add(`src/${mod}`);
    } else if (file.startsWith('e2e/')) {
      modules.add('e2e');
    } else if (file.startsWith('tests/')) {
      modules.add('tests');
    } else if (file.startsWith('tests-wasm/')) {
      modules.add('tests-wasm');
    } else if (file.startsWith('assets/')) {
      modules.add('assets');
    }
  }

  return {
    ...commit,
    type,
    category: CATEGORIES[type] || 'Other',
    scope,
    description,
    modules: Array.from(modules),
    files,
  };
}

// --- Output ---

function formatText(parsed) {
  const grouped = {};
  for (const commit of parsed) {
    if (!grouped[commit.category]) grouped[commit.category] = [];
    grouped[commit.category].push(commit);
  }

  let output = '';
  for (const [category, commits] of Object.entries(grouped)) {
    output += `\n### ${category}\n\n`;
    for (const c of commits) {
      const scope = c.scope ? `**${c.scope}**: ` : '';
      const modules = c.modules.length > 0 ? ` (${c.modules.join(', ')})` : '';
      output += `- ${scope}${c.description}${modules} — \`${c.short}\`\n`;
    }
  }
  return output.trim();
}

// --- Main ---

function main() {
  const args = process.argv.slice(2);
  const jsonOutput = args.includes('--json');
  const sinceArg = args.find(a => a.startsWith('--since='));
  const since = sinceArg ? sinceArg.split('=')[1] : null;

  const commits = getCommits(since);
  if (commits.length === 0) {
    console.log('No commits found.');
    return;
  }

  const parsed = commits.map(parseCommit);

  if (jsonOutput) {
    console.log(JSON.stringify(parsed, null, 2));
  } else {
    console.log(`Parsed ${parsed.length} commits${since ? ` since ${since}` : ''}:\n`);
    console.log(formatText(parsed));
  }
}

main();
