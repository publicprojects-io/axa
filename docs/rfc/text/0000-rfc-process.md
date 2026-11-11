# RFC: RFC Process

**RFC Number**: 0000
**Author**: axa maintainers
**Date**: 2026-11-11
**Status**: Accepted

---

## Summary

This RFC establishes the Request for Comments (RFC) process for proposing and discussing major changes to axa. This provides a structured way to propose, discuss, and implement significant changes while maintaining axa's principles and ensuring community input.

## Motivation

As axa grows, we need a formal process for:
- Proposing major changes
- Gathering community feedback
- Documenting decisions
- Ensuring alignment with axa's principles

The RFC process is common in open-source projects and provides a proven way to manage significant changes.

## Background

axa already has a specification-first approach where behavior changes require spec edits, code edits, and test edits. The RFC process complements this by providing a way to discuss and refine proposals before implementation.

Many successful open-source projects use RFC processes, including:
- Rust (https://github.com/rust-lang/rfcs)
- Python (https://github.com/python/peps)
- Node.js (https://github.com/nodejs/node/tree/main/doc/rfcs)

## Proposal

### RFC Process Overview

1. **Pre-RFC Discussion** (Optional): Discuss ideas informally
2. **Create RFC**: Write and submit an RFC document
3. **RFC Review**: Community and maintainers review the RFC
4. **Implementation**: Implement the accepted RFC
5. **Acceptance**: RFC is merged and marked as Accepted

### RFC Lifecycle

```
┌─────────────┐     ┌─────────────┐     ┌─────────────────┐
│   Idea       │────▶│   Draft     │────▶│    Active      │
└─────────────┘     └─────────────┘     │   (Review)      │
                                              │
                    ┌─────────────────────────┼─────────────────┐
                    │                         │                 │
                    ▼                         ▼                 ▼
            ┌─────────────┐         ┌─────────────┐     ┌─────────────┐
            │  Withdrawn  │         │  Rejected   │     │  Accepted   │
            └─────────────┘         └─────────────┘     └─────────────┘
                                                                  │
                                                                  ▼
                                                         ┌─────────────────┐
                                                         │ Implementation  │
                                                         └─────────────────┘
```

### RFC Structure

RFC documents are stored in `docs/rfc/text/` with numbered filenames (e.g., `NNNN-short-name.md`).

Each RFC must include:
- RFC number (assigned by maintainers)
- Author name and GitHub username
- Date
- Status
- Summary
- Motivation
- Proposal (with design details)
- Alternatives considered
- Open questions
- Implementation plan

### RFC Status

| Status | Description |
|--------|-------------|
| **Draft** | RFC is being written, not ready for review |
| **Active** | RFC is under active review |
| **Accepted** | RFC has been accepted and is being/has been implemented |
| **Rejected** | RFC has been rejected |
| **Withdrawn** | RFC author has withdrawn the proposal |
| **Superseded** | RFC has been replaced by a newer RFC |

### Review Process

1. The author submits an RFC PR
2. The maintainers assign an RFC number
3. The RFC enters **Active** status
4. The review team provides feedback for at least 7 days
5. The author addresses feedback and revises the RFC
6. The maintainers decide to Accept or Reject the RFC

### Acceptance Criteria

An RFC is accepted when:
- ✅ All review team concerns have been addressed
- ✅ The RFC aligns with axa's principles
- ✅ There is consensus among the maintainers
- ✅ The RFC follows the template
- ✅ The RFC is complete and well-written

### Axia's Principles

All RFCs must align with axa's core principles:

1. **Library, not framework**: axa is a library, not a framework
2. **Spec-first**: Code conforms to the specification
3. **Fail-closed**: Production refuses unsafe configurations
4. **Audit-friendly**: Every request path is traceable
5. **Zero indirection**: No macros, no code generation, no DI containers

### Implementation Requirements

Accepted RFCs must be implemented following axa's spec-first discipline:

1. **Spec edit**: Update `spec/index.md` with new requirements
2. **Code edit**: Implement the changes
3. **Test edit**: Add tests for the new functionality

Each spec requirement should name its test.

## Design

### Directory Structure

```
docs/rfc/
├── index.md              # This document
└── text/
    ├── 0000-rfc-process.md  # This RFC
    ├── 0001-*.md          # Future RFCs
    └── NNNN-*.md          # Future RFCs
```

### Filename Format

RFC filenames follow the pattern:
```
NNNN-short-description.md
```

Where:
- `NNNN` is a 4-digit number (zero-padded)
- `short-description` is a hyphen-separated description

### Metadata Format

Each RFC starts with metadata in the format:

```markdown
# RFC: [Short Title]

**RFC Number**: NNNN
**Author**: [Your Name] (@github-username)
**Date**: YYYY-MM-DD
**Status**: Draft
**Superseded by**: [optional]
```

## Specification Changes

This RFC adds the following to `spec/index.md`:

```
### RFC Process

RFC-0: The RFC process is defined in [docs/rfc/index.md](docs/rfc/index.md).
```

## Alternatives Considered

1. **No formal process**: Continue with informal discussions and direct PRs
   - Rejected: Lacks structure for major changes

2. **GitHub Issues only**: Use GitHub Issues for all proposals
   - Rejected: Issues are not suitable for detailed design discussions

3. **GitHub Discussions only**: Use GitHub Discussions for all proposals
   - Rejected: Discussions are ephemeral and hard to track

4. **Separate repository**: Maintain RFCs in a separate repository
   - Rejected: Keeps RFCs close to the code they affect

## Open Questions

None at this time.

## Implementation Plan

1. ✅ Create `docs/rfc/index.md` with RFC process documentation
2. ✅ Create `docs/rfc/text/0000-rfc-process.md` as the first RFC
3. ✅ Update `llms.txt` and `llms.json` with RFC documentation
4. Update `spec/index.md` to reference the RFC process

## References

- Rust RFC Process: https://github.com/rust-lang/rfcs
- Python PEP Process: https://github.com/python/peps
- Node.js RFC Process: https://github.com/nodejs/node/tree/main/doc/rfcs

---

## Decision Log

| Date | Decision | By | Notes |
|------|----------|----|-------|
| 2026-11-11 | RFC 0000 created | axa maintainers | Initial RFC process |
| 2026-11-11 | RFC 0000 accepted | axa maintainers | Approved and implemented |
