# File History Refinement

Investigation into what `file_history` should return, derived from analysis of actual nmem data. Spun out of the LSP investigation (Issue #9) after discovering the underlying data problem.

## Problem

`file_history` returns raw observation events — every read, edit, write, and search hit for a file across sessions. For a hot file like `s1_record.rs` (35 sessions, 209 touches), this produces ~2,236 tokens of mostly redundant read/edit/read/edit sequences. The signal-to-noise ratio makes the tool useless in practice.

Evidence: `file_history` has been called 3 times across 98 sessions. Only 1 was agent-initiated. The CLAUDE.md retrieval trigger ("first contact with a file → run `file_history`") has a 0.13% autonomous compliance rate. The tool exists but nobody uses it because the output isn't worth the cost.

## Current state of file observations

From the nmem database (5,929 total observations, 2,804 with file paths, 383 distinct files, 98 sessions):

| obs_type | count | pct | signal value |
|----------|-------|-----|--------------|
| `file_read` | 1,369 | 48.8% | None. The model reads constantly. |
| `file_edit` | 796 | 28.4% | Varies by context. |
| `search` | 420 | 15.0% | None. Grep hits aren't interventions. |
| `file_write` | 219 | 7.8% | Higher — creation is deliberate. |

Of the 796 edits: 77% agent-prompted, 19% user-prompted, 4% no prompt link. Both carry intent — the agent's thinking block often has the real reasoning.

### Intervention vs observation

The analogy: looking at a clock 50 times is not important. Changing the batteries is.

- **Observations** (reads, search hits): the model looked at the file. Not meaningful.
- **Interventions** (edits, writes): the model changed the file. Potentially meaningful.

2,804 file observations → 1,015 interventions (edits + writes). A 64% reduction by dropping reads and searches.

### Dedup within sessions

1,015 interventions collapse to **420 unique file-session pairs**. More than half are duplicate edits to the same file under the same prompt in the same session.

### Last intervention wins

Of multiple edits to a file in a session, only the last one matters — it's the final state. Prior edits are drafts. Exception: if the last intervention is associated with a failure (episode with `failures > 0` in `phase_signature`), the failure context should be preserved because the final state may be the broken state, not the resolution.

## Recording criteria

A file observation is worth recording when:

1. **The file was edited or written** — reads and search hits are never recorded.
2. **It's the last intervention per file per session** — intermediate edits are drafts.
3. **Exception: failure association** — if the file appears in an episode's `hot_files` where `failures > 0`, preserve the failure context alongside the last intervention.

This reduces 2,804 file observations to ~420 records, each representing "this file was changed in this session, here's the final context."

## What the refined record should contain

For each file-session pair:

| Field | Source | Purpose |
|-------|--------|---------|
| `file_path` | observation | Which file |
| `session_id` | observation | Which session |
| `timestamp` | last intervention | When (final edit) |
| `obs_type` | last intervention | edit vs write (creation) |
| `intent` | nearest prompt (user or agent) | Why the file was changed |
| `session_intent` | session summary | What the session was doing |
| `had_failures` | work_units.phase_signature | Whether this file was involved in a failure episode |
| `episode_intent` | work_units.intent | What the episode was trying to do (if failure) |

## Open questions

1. **Where does this live?** Options: (a) change what `s1_record.rs` writes to the observations table (stop recording reads/searches), (b) keep raw observations but build a materialized view/query for `file_history`, (c) build a separate `file_interventions` table populated at session end.

2. **Retention interaction.** If reads are no longer recorded, `s3_sweep.rs` retention rules for `file_read` become irrelevant. If the refined records are denser and more valuable, their TTL should be longer.

3. **What does the refined `file_history` output look like?** The current 2,236 tokens for `s1_record.rs` should compress to something much smaller. Need to prototype the query and measure.

4. **Should `file_history` be the only consumer?** The same refined data could feed SessionStart context injection (file alerts), a future LSP diagnostic generator, or episode narratives.

## Data supporting this analysis

### File touch frequency across sessions

| Bucket | Files | Pct |
|--------|-------|-----|
| 1 session only | 240 | 62.7% |
| 2-3 sessions | 70 | 18.3% |
| 4-10 sessions | 51 | 13.3% |
| 10+ sessions | 22 | 5.7% |

### Prior history rate

- **66.8%** of file-session pairs involve files with prior session history.
- In recent nmem sessions, **83-100%** of files touched per session have prior history.
- Per observation: **74.6%** of file observations are on files with prior history.

### MCP tool usage

| Tool | Calls |
|------|-------|
| `search` | 72 |
| `session_summaries` | 16 |
| `get_observations` | 16 |
| `recent_context` | 8 |
| `current_stance` | 5 |
| `create_marker` | 5 |
| `session_trace` | 4 |
| **`file_history`** | **3** |
| `timeline` | 2 |
| `regenerate_context` | 2 |

Of the 3 `file_history` calls: 2 were user-directed ("use nmem to search", "check the file history"), 1 was agent-initiated (investigating a GitHub issue, called `file_history` on `s5_project.rs` before editing).

## Sources

- nmem database: `~/.nmem/nmem.db` (5,929 observations, 98 sessions)
- LSP investigation: `design/lsp-investigation.md`
- GitHub Issue #9: LSP server proposal
