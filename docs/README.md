# Gestalt Documentation

This index defines how documentation in this repository is classified, which
documents are authoritative, and how existing large or overlapping documents
should be simplified.

## Authority

When documents conflict, use this order:

1. **Accepted ADRs** define architectural decisions.
2. **Released version contracts** under `docs/v0.1/` define supported behavior
   and compatibility promises for that release.
3. **Active feature specifications** define approved or proposed feature scope
   without overriding ADRs.
4. **Domain guides and references** explain how to use implemented behavior.
5. **Implementation plans** describe execution; they do not create
   architecture.
6. **PRD and architecture overview** define product direction and system
   context.
7. **Audits, solutions, and archived documents** are point-in-time evidence and
   history.

Source code and tests determine what is currently implemented. A document does
not become a released contract until its behavior is implemented, tested, and
published in the relevant version-contract documentation.

## Document Types

| Type | Purpose | Expected lifecycle |
|---|---|---|
| ADR | Record one architectural decision and its consequences | proposed → accepted → superseded |
| Decision record | Record a coordinated set of release-gating decisions | proposed → accepted → superseded |
| Version contract | Define released API, schema, CLI, or persistence behavior | active for the supported release |
| Feature specification | Define goals, invariants, contracts, and acceptance criteria | proposed → active → implemented/superseded |
| Implementation plan | Break an approved specification into executable tasks | proposed → active → completed/abandoned |
| PRD / architecture | Define product direction or current system context | active → superseded |
| Guide | Explain an implemented workflow | active → deprecated |
| Reference | Describe an implemented protocol or schema | active and versioned |
| Audit | Record evidence and findings at a date | immutable point-in-time record |
| Solution | Record how a finding was resolved | historical with links to enforcement |
| Migration | Explain a version transition | active while either version is supported |
| Inventory / tracker / index | Record repository-wide classification or execution state | active → completed/historical |
| Archive | Preserve superseded context | historical |

The authoritative metadata for every maintained Markdown file is the
[documentation inventory](./plans/v0.1-hardening/documentation-inventory.md).
Documents may repeat it as YAML frontmatter, but duplicated frontmatter is not
required. Each inventory row must declare:
- `status`: One of the allowed statuses for the document type (e.g., proposed, active, accepted, superseded, completed, abandoned).
- `type`: The document category (e.g., adr, feature-spec, plan, guide, reference).
- `target`: The target release version (e.g., v0.1, v0.2, general).
- `owners`: The primary owning team or individuals responsible for maintenance.

### Owner Responsibilities
1. **Maintenance:** Keep the document content aligned with current code implementation or active plans.
2. **Review:** Approve any changes to downstream documents relying on this document's domain.
3. **Maturity Updates:** Transition document status as the lifecycle changes (e.g., proposed -> active -> completed).
4. **Link Preservation:** Verify relative links when moving files, ensuring redirects are created at the old paths.

### Archive and Redirect Rules
1. **Never Silent Deletes:** Historical plans or specs must not be silently deleted if they are referenced.
2. **Redirect Stub:** Replace the contents of the moved file with a notice that links to its repository-relative replacement.
3. **Archive Marker:** Historical or superseded documents must be marked `status: archived` and stored under `docs/archive/` (or labeled clearly if left in place).


## Canonical Domain Map

| Domain | Current authority | Supporting documents | Required action |
|---|---|---|---|
| Product philosophy and scope | `gestalt-harness-prd.md` | root `README.md` | Simplify to vision, boundaries, users, and priorities |
| System architecture | accepted ADRs | `gestalt-harness-architecture.md` | Reduce to an overview and links |
| Runtime composition | ADR-023 | architecture overview | Keep ADR authoritative |
| Configuration | ADR-025 as amended by ADR-031, plus implemented schema | `feature-spec/config-extension.md` | Remove legacy TOML and aliases; reconcile remaining scope |
| Context projection | accepted context ADRs plus active hardening spec | context feature specs and plans | Consolidate overlapping invariants into one domain contract |
| MCP | ADR-027 | `mcp-client-best-practices.md` | Keep guide implementation-focused |
| Extension packages | ADR-028 | product-neutral extension spec | Treat broad spec as evolution, not current authority |
| Runtime snapshots | ADR-029, clarified by H0B-F06 | extension spec | Keep the lease boundary aligned with the assistant-turn boundary |
| Lifecycle protocol | ADR-030 as amended by ADR-031 | JSON-RPC and extension guides | Make V2 exclusive and remove V1 compatibility |
| Greenfield compatibility cutoff | ADR-031 | v0.1 hardening spec | Maintain removal ledger and absence checks |
| Crate boundaries | implemented workspace plus crate-boundary spec | crate READMEs | Mark consolidation spec implemented and remove old crate names from active docs |
| v0.1 hardening | `feature-spec/v0.1-hardening.md` | future phase plans | Use as release-scope specification |

## Existing Document Disposition

### Root and monolithic documents

- `README.md` remains the concise user-facing entry point.
- `gestalt-harness-prd.md` should retain product vision, philosophy, target
  users, scope boundaries, and priorities. Move technical contracts and
  implementation details to domain documents.
- `gestalt-harness-architecture.md` should become a current architecture
  overview. Extract embedded accepted ADRs 001-022 into standalone files before
  deleting their source sections.
- `gestalt-harnes-implementation-roadmap.md` contains a filename typo and a
  historical cross-domain plan. Replace it with smaller release/feature plans,
  then archive it with a redirect rather than silently renaming or deleting it.

### Feature specifications

Feature specifications should describe durable behavior:

- invariants;
- public or persisted contracts;
- failure and security semantics;
- non-goals;
- compatibility;
- acceptance criteria.

They should not remain long-lived mirrors of source trees or current Rust
structs. Specs that name crates removed by consolidation must be updated before
they are treated as active.

The context documents currently overlap. Their durable decisions should be
consolidated into one context architecture contract, while algorithm-specific
work remains in focused specs.

The extension documents also overlap. ADR-028 through ADR-031 are normative.
The product-neutral extension specification describes broader evolution;
manifest, JSON-RPC, permissions, and authoring documents should each remain
focused references or guides.

### Plans, audits, and solutions

- Plans must link to the specification and ADRs they implement.
- Completed plans should be marked completed; abandoned plans should record why.
- Audits remain dated point-in-time evidence and should not be rewritten as
  current architecture.
- Solution documents should link to the ADR, tests, or code that keeps the
  solution enforced.

## Simplification Workflow

Documentation cleanup is incremental:

1. **Inventory:** assign type, owner, status, domain, and authority.
2. **Declare authority:** fix metadata and point each domain at one canonical
   document.
3. **Extract decisions:** move accepted decisions from monoliths and feature
   specs into ADRs.
4. **Reduce monoliths:** replace duplicated detail with short explanations and
   links.
5. **Harden domains:** reconcile overlapping specs and remove stale code
   snapshots.
6. **Archive safely:** preserve history and add redirects where paths are known
   to be referenced.

Do not perform a bulk move or deletion without validating internal links and
confirming the replacement document.

## Version Contract Documentation

`docs/v0.1/` will contain only implemented, test-backed compatibility
documentation. Proposed behavior stays in feature specifications until it
passes its release criteria.

The intended v0.1 contract map includes:

- embedding and runtime control;
- app services;
- context diagnostics;
- trace and run manifests;
- configuration;
- policy and approval;
- extension compatibility;
- stable CLI automation.

## Maintenance Rules

1. New architecture decisions require ADRs.
2. Feature specs cite the ADRs that constrain them.
3. Plans cannot override specs or ADRs.
4. Examples in active documents must match the current crate layout and schema.
5. Historical source snapshots are dated and labeled.
6. Moved documents leave redirects when links would otherwise break.
7. Internal Markdown links are validated during documentation hardening.
8. A completed implementation updates its feature-spec status and relevant
   version contract or guide.
