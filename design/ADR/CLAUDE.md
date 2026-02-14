# ADR Directory

## Purpose

Architecture Decision Records for the nmem project. These documents capture design decisions, trade-offs, and open questions for a cross-session memory system built on Viable System Model principles.

## Approach

These ADRs are developed collaboratively during nmem's early design phase. The goal is not to reach conclusions quickly but to surface the right questions before committing to implementation.

### Adversarial refinement

Each ADR uses an adversarial framing: push assumptions to their extremes and see where they break. Rather than arguing for a preferred approach, force the strongest case against it. "What if we never use an LLM for extraction?" is more useful than "we should probably use structured extraction." The point is to find the load-bearing assumptions — the ones where being wrong is expensive to recover from.

### Questions over answers

At this stage of design, an insightful question is worth more than a premature answer. Annotations and open questions should challenge practical assumptions against the actual scope of the project: a local-first, single-developer tool for session memory. Generic enterprise patterns (multi-region DR, team training, 1000 writes/sec scaling) are noise unless grounded in nmem's real constraints.

### Dependency ordering

ADRs have explicit dependency chains. Some decisions unlock others — extraction strategy (002) is the root because it determines data volume, schema shape, and whether vector search is even needed. Others are independent (006, interface protocol). Work the chain from root decisions outward.

## Conventions

- Original generated text is never modified — corrections and challenges are added as blockquote annotations (`> **[ANNOTATION]**` or `> **[Q:]**`)
- Claims are verified against primary sources (official docs, GitHub issues) before being accepted
- Each annotation includes the date and source of verification where applicable
