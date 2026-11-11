# RFC Process

This document describes the Request for Comments (RFC) process for proposing and discussing major changes to axa.

## Overview

The RFC process is used for:
- New features that change axa's behavior
- Breaking changes to existing functionality
- Architectural decisions
- New crates in the axa family
- Significant changes to the specification

Small changes (bug fixes, documentation improvements, etc.) do not require an RFC.

## When to Use RFC

| Type of Change | RFC Required? | Notes |
|----------------|---------------|-------|
| Bug fixes | No | Direct PR is fine |
| Documentation updates | No | Direct PR is fine |
| Test improvements | No | Direct PR is fine |
| New feature (minor) | Maybe | If it adds significant functionality |
| New feature (major) | Yes | Always for major features |
| Breaking change | Yes | Always for breaking changes |
| New crate | Yes | Always for new crates |
| Spec changes | Yes | Always for spec changes |
| Architectural changes | Yes | Always for architectural changes |

## RFC Lifecycle

### 1. Pre-RFC Discussion (Optional)

Before writing a full RFC, you can:
- Open a discussion on GitHub Discussions
- Discuss in the community chat
- Get feedback on your idea

This helps ensure your proposal has support before investing in a full RFC.

### 2. Create RFC

1. **Fork the repository** and create a new branch
2. **Create a new RFC document** in `docs/rfc/text/` with a numbered filename (e.g., `0001-feature-name.md`)
3. **Use the RFC template** below
4. **Submit a PR** with your RFC

### 3. RFC Review

- The maintainers will review the RFC
- Community members can provide feedback
- The RFC will be discussed and revised as needed
- The PR will be merged when the RFC is ready for implementation

### 4. Implementation

- The RFC author or a volunteer implements the RFC
- The implementation follows the spec-first discipline:
  - Spec edit
  - Code edit
  - Test edit
- The implementation PR references the RFC

### 5. Acceptance

- The implementation PR is reviewed and merged
- The RFC is marked as **Accepted**
- The feature is included in the next release

### 6. Post-Implementation

- If issues are found, the RFC can be amended
- If the RFC is found to be flawed, it can be reverted

## RFC Template

```markdown
# RFC: [Short Title]

**RFC Number**: [Assigned by maintainers]
**Author**: [Your Name]
**Date**: [YYYY-MM-DD]
**Status**: Draft / Active / Accepted / Rejected / Withdrawn
**Superseded by**: [RFC number if applicable]

---

## Summary

[One paragraph explanation of the proposal]

## Motivation

[Why are we doing this? What problem does it solve?]

## Background

[Any necessary background information, context, or prior art]

## Proposal

[Detailed description of the proposed change]

### Design

[Technical design details]

### Specification Changes

[Changes to `spec/index.md` if applicable]

### API Changes

[Changes to public API]

### Breaking Changes

[List any breaking changes and migration paths]

### Dependencies

[New dependencies required]

## Alternatives Considered

[Other approaches that were considered and why they were rejected]

## Open Questions

[Questions that need to be answered before implementation]

## Implementation Plan

[Steps to implement this RFC]

## References

[Links to related issues, PRs, discussions, etc.]

---

## Decision Log

| Date | Decision | By | Notes |
|------|----------|----|-------|
| | | | |
```

## RFC Format

### Filename

RFC documents are stored in `docs/rfc/text/` with filenames in the format:
```
NNNN-short-name.md
```

Where `NNNN` is a 4-digit number (e.g., `0001-feature-name.md`).

### Numbering

RFC numbers are assigned by the maintainers in the order they are accepted for review.

### Status

Each RFC has one of the following statuses:

| Status | Description |
|--------|-------------|
| **Draft** | RFC is being written and is not ready for review |
| **Active** | RFC is under active review |
| **Accepted** | RFC has been accepted and is being or has been implemented |
| **Rejected** | RFC has been rejected |
| **Withdrawn** | RFC author has withdrawn the proposal |
| **Superseded** | RFC has been replaced by a newer RFC |

### Metadata

Each RFC must include the following metadata at the top:

```markdown
# RFC: [Short Title]

**RFC Number**: NNNN
**Author**: [Your Name] (@github-username)
**Date**: YYYY-MM-DD
**Status**: Draft
```

## Review Process

### RFC Review Team

The RFC review team consists of:
- The axa maintainers
- Community members with expertise in the relevant area
- Anyone who wants to participate

### Review Criteria

RFCs are evaluated based on:

1. **Alignment with axa principles**: Does the RFC follow axa's philosophy?
   - Library, not framework
   - Spec-first
   - Fail-closed
   - Audit-friendly
   - Zero indirection

2. **Technical merit**: Is the proposal well-designed?

3. **Backward compatibility**: Does the RFC break existing functionality?

4. **Maintenance burden**: Will this RFC increase maintenance burden?

5. **Community support**: Is there support for this RFC in the community?

### Review Period

- The RFC will be in **Active** status for at least 7 days
- The review team will provide feedback during this period
- The RFC author should address feedback and revise the RFC as needed

### Acceptance Criteria

An RFC is accepted when:
- All review team concerns have been addressed
- The RFC aligns with axa's principles
- There is consensus among the maintainers

### Rejection

An RFC can be rejected if:
- It doesn't align with axa's principles
- It has fundamental design flaws
- It lacks community support
- The author fails to address feedback

Rejected RFCs can be revised and resubmitted.

## Implementation Guidelines

### Spec-First

All RFC implementations must follow the spec-first discipline:

1. **Spec edit**: Update `spec/index.md` with the new requirements
2. **Code edit**: Implement the changes in code
3. **Test edit**: Add tests for the new functionality

Each spec requirement should name its test.

### Backward Compatibility

If an RFC introduces breaking changes:
- Provide a migration path
- Document the breaking changes clearly
- Consider a deprecation period

### Documentation

RFC implementations must include:
- Documentation in the appropriate location
- Updates to `llms.txt` and `llms.json` if adding new documentation
- Examples if applicable

## RFC Index

| Number | Title | Author | Date | Status |
|--------|-------|--------|------|--------|
| [0000](text/0000-rfc-process.md) | RFC Process | axa maintainers | 2026-11-11 | Accepted |

[Add new RFCs here as they are created]

## Example RFCs

- [RFC 0000: RFC Process](text/0000-rfc-process.md) - This document

## FAQ

### Who can submit an RFC?

Anyone can submit an RFC. You don't need to be a maintainer or a contributor.

### Do I need to implement the RFC myself?

No. You can submit an RFC without implementing it. However, RFCs with implementations are more likely to be accepted.

### What if my RFC is rejected?

Rejected RFCs can be revised and resubmitted. The review team will provide feedback on why the RFC was rejected.

### Can an RFC be changed after acceptance?

Yes. If issues are found during implementation, the RFC can be amended. However, significant changes may require a new RFC.

### What is the relationship between RFCs and the spec?

RFCs propose changes to the spec. Once an RFC is accepted and implemented, the spec is updated to reflect the changes.

## Contact

For questions about the RFC process:
- Open a GitHub Discussion
- Ask in the community chat
- Contact the maintainers
