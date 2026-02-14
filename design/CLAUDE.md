# nmem

Autonomous cross-session memory system. Successor to claude-mem.

## Project
- **Location**: `~/forge/nmem/`
- **Design doc**: `DESIGN.md`
- **Status**: Early design phase — no implementation yet
- **Organizing principle**: Viable System Model (VSM)

## Predecessor Observations (claude-mem)

Data extracted from claude-mem's own database. These illustrate both what the system captured and its failure modes.

### Quality Issues Discovered
When tested with local models (LM Studio), the SDK agent hallucinated freely — fabricating observations about nonexistent files and work that never happened. Only 2 of 9 observations contained real data. The architecture had **no validation or guardrails** to reject bad data regardless of source.

This exposed a fundamental design flaw: the system's data integrity depends entirely on the quality of a single LLM component, with no verification layer. A viable system must be resilient to component degradation.

### Real Observations
| ID | Date | Type | Summary |
|----|------|------|---------|
| #2 | Feb 7 | discovery | Grep search for `parseObservations` in claude-mem worker source (ResponseProcessor.ts) |
| #3 | Feb 8 | feature | Read Ghostty config from `~/.config/ghostty/config` (real action, but fabricated file modifications) |

### Fabricated Observations (examples of failure)
| ID | Type | Fabrication |
|----|------|-------------|
| #1 | bugfix | "Fixed memory leak in database connection pool" — never happened |
| #4 | feature | "Email Notifications" — never happened |
| #5-8 | mixed | OAuth2/PKCE authentication work — entirely fabricated |
| #9 | bugfix | "Database connection issue in production" — never happened |

### Session Summaries
| ID | Date | Summary |
|----|------|---------|
| #S1-S5 | Feb 7-8 | Current session activity (Ghostty config, nmem design) |

### Key Lessons for nmem
1. **LLM extraction is unreliable** — the observer agent hallucinates freely when given raw tool output
2. **Cost is high for garbage** — ~1700 tokens per observation, mostly fabricated
3. **No validation layer** — nothing checks if referenced files actually exist
4. **No feedback loop** — the system never learns which observations were useful vs noise
5. **Structured extraction > LLM extraction** for factual data (files read, commands run, errors encountered)
6. **LLM should be reserved for synthesis** (S4/intelligence), not inline fact extraction (S1/operations)
