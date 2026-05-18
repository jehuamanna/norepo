#!/usr/bin/env node

/**
 * changelog-generator.js — Generate structured changelog from Git history
 *
 * Parses commits, groups by type and date, and updates changelog.md.
 *
 * Usage:
 *   node instructions/automation/changelog-generator.js [--since=<date>]
 */

const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const ROOT = path.resolve(__dirname, '..', '..');
const CHANGELOG_PATH = path.join(ROOT, 'instructions', 'changelog.md');

const CATEGORIES = {
  feat: 'Features',
  fix: 'Fixes',
  docs: 'Documentation',
  refactor: 'Refactors',
  test: 'Tests',
  chore: 'Chores',
  perf: 'Performance',
};

function getCommits(since) {
  const sinceArg = since ? `--since="${since}"` : '--max-count=200';
  try {
    const log = execSync(
      `git -C "${ROOT}" log ${sinceArg} --pretty=format:"%h|%s|%an|%ai" --no-merges`,
      { encoding: 'utf-8', maxBuffer: 10 * 1024 * 1024 }
    );
    return log.trim().split('\n').filter(Boolean).map(line => {
      const [short, subject, author, date] = line.split('|');
      return { short, subject, author, date: date.split(' ')[0] };
    });
  } catch {
    return [];
  }
}

function categorize(subject) {
  const match = subject.match(/^(\w+)(?:\([^)]*\))?:\s*(.+)$/);
  if (match) {
    const type = match[1].toLowerCase();
    return {
      type,
      category: CATEGORIES[type] || 'Other',
      description: match[2],
    };
  }

  const lower = subject.toLowerCase();
  if (lower.includes('fix')) return { type: 'fix', category: 'Fixes', description: subject };
  if (lower.includes('add') || lower.includes('feat')) return { type: 'feat', category: 'Features', description: subject };
  if (lower.includes('refactor')) return { type: 'refactor', category: 'Refactors', description: subject };
  if (lower.includes('test')) return { type: 'test', category: 'Tests', description: subject };
  if (lower.includes('doc')) return { type: 'docs', category: 'Documentation', description: subject };
  return { type: 'chore', category: 'Chores', description: subject };
}

function generateChangelog(commits) {
  // Group by date
  const byDate = {};
  for (const commit of commits) {
    if (!byDate[commit.date]) byDate[commit.date] = [];
    const cat = categorize(commit.subject);
    byDate[commit.date].push({
      ...commit,
      ...cat,
    });
  }

  let sections = '';
  for (const [date, dateCommits] of Object.entries(byDate)) {
    // Group by category within date
    const byCategory = {};
    for (const c of dateCommits) {
      if (!byCategory[c.category]) byCategory[c.category] = [];
      byCategory[c.category].push(c);
    }

    sections += `\n### ${date}\n`;
    for (const [category, catCommits] of Object.entries(byCategory)) {
      sections += `\n**${category}**\n\n`;
      for (const c of catCommits) {
        sections += `- ${c.description} (\`${c.short}\`)\n`;
      }
    }
  }

  return sections;
}

function main() {
  const args = process.argv.slice(2);
  const sinceArg = args.find(a => a.startsWith('--since='));
  const since = sinceArg ? sinceArg.split('=')[1] : null;

  const commits = getCommits(since);
  if (commits.length === 0) {
    console.log('No commits found.');
    return;
  }

  console.log(`Processing ${commits.length} commits...`);
  const newEntries = generateChangelog(commits);

  // Read existing changelog
  let changelog = fs.readFileSync(CHANGELOG_PATH, 'utf-8');

  // Replace the "Unreleased" section
  const unreleased = `## Unreleased\n${newEntries}`;
  changelog = changelog.replace(
    /## Unreleased[\s\S]*?(?=\n## |\n---\n## |$)/,
    unreleased + '\n'
  );

  // Update timestamp
  const today = new Date().toISOString().split('T')[0];
  changelog = changelog.replace(
    /\*Last updated: \d{4}-\d{2}-\d{2}\*/,
    `*Last updated: ${today}*`
  );

  fs.writeFileSync(CHANGELOG_PATH, changelog, 'utf-8');
  console.log(`Updated: changelog.md (${commits.length} commits)`);
}

main();
