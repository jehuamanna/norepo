# Documentation Index

> Auto-updating master index for Operon developer documentation.

*Last updated: 2026-05-14*

---

## Navigation

### Core Documentation

- [README.md](README.md) — Entry point, onboarding, quick links
- [requirements.md](requirements.md) — Business and functional requirements
- [architecture.md](architecture.md) — System design and module boundaries
- [how-it-works.md](how-it-works.md) — Application workflows
- [tech-stack.md](tech-stack.md) — Technology choices and rationale
- [folder-structure.md](folder-structure.md) — Project layout

### Setup & Build

- [setup-guide.md](setup-guide.md) — General setup overview
- [setup-windows.md](setup-windows.md) — Windows-specific setup
- [setup-linux.md](setup-linux.md) — Linux-specific setup
- [setup-macos.md](setup-macos.md) — macOS-specific setup
- [build-guide.md](build-guide.md) — Build pipeline
- [deployment-guide.md](deployment-guide.md) — Deployment architecture

### Development

- [development-guide.md](development-guide.md) — Local workflow and tooling
- [api-reference.md](api-reference.md) — REST API documentation
- [database-schema.md](database-schema.md) — Schema and migrations
- [environment-variables.md](environment-variables.md) — Configuration reference

### Standards & Quality

- [coding-guidelines.md](coding-guidelines.md) — Code standards
- [security-guidelines.md](security-guidelines.md) — Security practices
- [testing-guide.md](testing-guide.md) — 4-tier test strategy
- [troubleshooting.md](troubleshooting.md) — Common issues
- [performance-optimization.md](performance-optimization.md) — Performance tuning

### Planning

- [future-improvements.md](future-improvements.md) — Roadmap and technical debt
- [changelog.md](changelog.md) — Version history

### Maintenance

- [coverage-report.md](coverage-report.md) — Documentation coverage
- [docs-maintenance-guide.md](docs-maintenance-guide.md) — How to maintain docs

### Diagrams

- [diagrams/architecture-flow.md](diagrams/architecture-flow.md)
- [diagrams/request-lifecycle.md](diagrams/request-lifecycle.md)
- [diagrams/database-relations.md](diagrams/database-relations.md)
- [diagrams/deployment-flow.md](diagrams/deployment-flow.md)

### Templates

- [templates/module-template.md](templates/module-template.md)
- [templates/api-template.md](templates/api-template.md)
- [templates/service-template.md](templates/service-template.md)

### Automation

- [automation/doc-sync.js](automation/doc-sync.js)
- [automation/commit-parser.js](automation/commit-parser.js)
- [automation/changelog-generator.js](automation/changelog-generator.js)
- [automation/coverage-checker.js](automation/coverage-checker.js)
- [automation/structure-scanner.js](automation/structure-scanner.js)
- [automation/documentation-map.json](automation/documentation-map.json)

---

## Documentation Coverage Summary

| Category | Documented | Total | Coverage |
|---|---|---|---|
| Crates | 16 | 16 | 100% |
| API Routes | 14 | 14 | 100% |
| Database Tables | 20 | 20 | 100% |
| Environment Variables | 12 | 12 | 100% |
| Setup Guides | 3 | 3 | 100% |

*Run `node instructions/automation/coverage-checker.js` to refresh.*

---

## Latest Changes

*Run `node instructions/automation/changelog-generator.js` to update.*
