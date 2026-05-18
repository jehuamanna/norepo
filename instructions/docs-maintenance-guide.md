# Documentation Maintenance Guide

## Overview

This documentation system is designed to stay synchronized with the codebase. It includes automation scripts for incremental updates and coverage tracking.

---

## Maintenance Workflow

### When to Update Docs

| Change Type | Docs to Update |
|---|---|
| New crate added | folder-structure.md, architecture.md, coverage-report.md |
| New API endpoint | api-reference.md |
| New database table/migration | database-schema.md |
| New environment variable | environment-variables.md |
| New feature | how-it-works.md, changelog.md |
| Dependency change | tech-stack.md |
| New test spec | testing-guide.md |
| Build process change | build-guide.md |
| Security change | security-guidelines.md |

### Automated Updates

Run automation scripts to sync documentation:

```bash
# Full sync (scans repo, updates all docs)
node instructions/automation/doc-sync.js

# Generate changelog from git commits
node instructions/automation/changelog-generator.js

# Check documentation coverage
node instructions/automation/coverage-checker.js

# Scan project structure
node instructions/automation/structure-scanner.js
```

---

## Automation Scripts

### `doc-sync.js`

Scans the repository and detects changes:
- New crates/modules
- New API routes
- New database migrations
- Modified file structure

Updates documentation links, timestamps, and navigation.

**Usage**:
```bash
node instructions/automation/doc-sync.js
```

### `commit-parser.js`

Parses Git commits and categorizes them:
- `feat` → Features
- `fix` → Fixes
- `docs` → Documentation
- `refactor` → Refactors
- `test` → Tests
- `chore` → Chores

**Usage**:
```bash
node instructions/automation/commit-parser.js
# Output: JSON array of categorized commits
```

### `changelog-generator.js`

Generates structured changelog from commit history:
- Groups by type
- Includes timestamps and commit references
- Updates `changelog.md`

**Usage**:
```bash
node instructions/automation/changelog-generator.js
```

### `coverage-checker.js`

Detects undocumented modules:
- Scans all crates
- Checks API routes against api-reference.md
- Checks database tables against database-schema.md
- Generates `coverage-report.md`

**Usage**:
```bash
node instructions/automation/coverage-checker.js
```

### `structure-scanner.js`

Scans project structure and updates `documentation-map.json`:
- Maps source files to documentation files
- Detects new modules
- Flags orphaned documentation

**Usage**:
```bash
node instructions/automation/structure-scanner.js
```

---

## Manual Updates

Some documentation requires manual attention:

### Always Manual

- `requirements.md` — Business decisions
- `future-improvements.md` — Roadmap planning
- `security-guidelines.md` — Security policy changes
- `coding-guidelines.md` — Team standards

### Semi-Automated

- `architecture.md` — Auto-detect new crates, manual diagram updates
- `how-it-works.md` — Auto-detect new features, manual workflow descriptions
- `troubleshooting.md` — Manual additions from support issues

---

## File Structure

```
instructions/
├── automation/
│   ├── doc-sync.js              # Main sync orchestrator
│   ├── commit-parser.js         # Git commit categorizer
│   ├── changelog-generator.js   # Changelog builder
│   ├── coverage-checker.js      # Coverage analyzer
│   ├── structure-scanner.js     # Project structure scanner
│   └── documentation-map.json   # Source-to-docs mapping
├── diagrams/                    # Mermaid diagram sources
├── templates/                   # Documentation templates
└── *.md                         # Documentation files
```

---

## CI/CD Integration

### Recommended CI Steps

```yaml
# GitHub Actions example
docs-check:
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - uses: actions/setup-node@v4
      with:
        node-version: '20'
    - name: Check documentation coverage
      run: node instructions/automation/coverage-checker.js --ci
    - name: Validate documentation links
      run: node instructions/automation/doc-sync.js --validate
```

### Git Hooks (Optional)

Add to `.git/hooks/pre-commit`:

```bash
#!/bin/bash
# Auto-update changelog on commit
node instructions/automation/changelog-generator.js
git add instructions/changelog.md
```

---

## Templates

Use templates for consistency when adding new documentation:

- [Module Template](templates/module-template.md) — for documenting new crates/modules
- [API Template](templates/api-template.md) — for documenting new API endpoints
- [Service Template](templates/service-template.md) — for documenting new services

---

## Markdown Standards

### Formatting

- Use ATX-style headings (`#`, `##`, `###`)
- Use tables for structured data
- Use code blocks with language identifiers
- Use Mermaid diagrams for visual explanations
- Cross-link related documents

### Example

```markdown
## Feature Name

**Location**: `crates/operon-feature/`

### Overview

Brief description.

### Configuration

| Variable | Default | Description |
|---|---|---|
| `VAR_NAME` | `value` | What it does |

### Usage

\`\`\`rust
// Code example
\`\`\`

### Related

- [Architecture](architecture.md) — system design context
- [API Reference](api-reference.md) — endpoint details
```

---

## Keeping Docs Accurate

1. **Review during PR**: Check if code changes affect any documentation
2. **Run coverage checker**: `node instructions/automation/coverage-checker.js`
3. **Update timestamps**: Automation scripts handle this
4. **Delete stale docs**: Remove documentation for deleted features
5. **Test commands**: Verify that documented commands actually work
