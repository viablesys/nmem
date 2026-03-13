# Dojo — AI-Assisted Skill Acquisition and Crystallization

Training construct for systematic skill loading, practice, and crystallization through cross-session memory. Built atop nmem's observation/episode/stance infrastructure. Practice generates observations; memory captures progression; crystallization externalizes tacit learning into reusable artifacts; feedback refines everything.

## Theoretical Foundations

### Dreyfus Model: Five Stages of Competence

Stuart and Hubert Dreyfus (1980). Progression from rule-based reasoning to intuitive performance.

| Stage | Characteristics | Dojo Behavior |
|-------|----------------|---------------|
| **Novice** | Context-free rules, slow | Full walkthroughs, worked examples, heavy scaffolding |
| **Advanced Beginner** | Recognizes patterns, connects context | Guided exercises, partial scaffolding |
| **Competent** | Routines, deliberate planning | Self-directed practice with feedback |
| **Proficient** | Intuition guides decisions | Real-world application, minimal scaffolding |
| **Expert** | Fluid unconscious performance | Teaching others, creating artifacts |

**nmem mapping**: Stance fingerprints correlate with Dreyfus stages:

| Dreyfus Stage | Phase | Scope | Locus | Novelty | Friction |
|---------------|-------|-------|-------|---------|----------|
| Novice | think | diverge | external | novel | high |
| Advanced Beginner | think | converge | mixed | mixed | medium |
| Competent | act | converge | internal | routine | medium |
| Proficient | act | converge | internal | routine | low |
| Expert | act | converge | internal | routine | very low |

Detection query:
```sql
SELECT phase, scope, locus, novelty,
       COUNT(*) FILTER(WHERE friction = 'friction') as friction_count,
       COUNT(*) as total
FROM observations
WHERE session_id IN (SELECT session_id FROM session_skills WHERE skill_id = ?)
GROUP BY phase, scope, locus, novelty;
```

### Deliberate Practice (Ericsson)

Activities specifically designed to improve current performance. Not repetition.

Core requirements:
- **Designed improvement**: Target specific aspects of performance
- **At the edge**: Operate in ZPD (uncomfortable)
- **Immediate feedback**: Errors identified and corrected in real-time
- **Repetition with variation**: Varied contexts, not rote
- **Overlearning**: Practice beyond initial proficiency — single most important retention factor

Critiques: effect sizes smaller than originally claimed. Cognitive ability explains significant variance. Necessary but not sufficient.

**nmem mapping**: High friction during deliberate practice is expected — it signals the learner is at capability edge. Track via `work_units.phase_signature` friction counts per skill-tagged episode.

### Zone of Proximal Development (Vygotsky)

Space between what learner can do unsupported and what they cannot do even with support.

**Scaffolding** (Bruner): temporary support, high when task is new, gradually withdrawn.

**nmem mapping**: 5D stance data provides real-time ZPD estimation. `think+diverge+novel+friction` = at/beyond edge. `act+converge+routine+smooth` = comfort zone. Target the transition.

### Cognitive Load Theory (Sweller)

Working memory limited (~7 items). Three load types:
- **Intrinsic**: Material complexity (manage through chunking)
- **Extraneous**: Poor design burden (minimize)
- **Germane**: Schema construction load (optimize)

**Expertise reversal** (Kalyuga et al., 2003): techniques effective for novices harm experts. Forced scaffolding wastes time and increases load for advanced learners.

**nmem mapping**: Friction on `routine` material suggests extraneous load (bad explanation). Friction on `novel` material is expected intrinsic load. Distinguish via novelty dimension.

### Bloom's Taxonomy and Mastery Learning

Cognitive hierarchy: Remember, Understand, Apply, Analyze, Evaluate, Create. Mastery learning (Bloom, 1968): 90%+ accuracy on prerequisites before advancing. Competency-based, not time-based.

**nmem mapping**: Competency gates map to assessment observations. Pass/fail per concept tracked in `session_skills`, fed to BKT.

### Cognitive Apprenticeship (Collins, Brown, Newman, 1989)

Six methods for making thinking visible:
1. **Modeling**: Expert demonstrates
2. **Coaching**: Observe student, offer hints
3. **Scaffolding**: Execute parts student cannot manage
4. **Fading**: Gradual support removal
5. **Articulation**: Student explains reasoning
6. **Reflection**: Student compares own process to expert

The existing Rust tutorial pattern is cognitive apprenticeship: AI explains (modeling), learner explains constructs (articulation), AI hints (coaching/scaffolding), across sessions AI reduces intervention (fading).

**nmem mapping**: Scaffolding level tracked per concept in markers. Fading detected when `scaffolding_needed` disappears from successive markers for same concept.

### ACT-R: Knowledge Compilation (Anderson)

Knowledge phases:
1. **Declarative**: Facts and rules (conscious, slow) — tutorials
2. **Procedural**: Compiled productions (faster) — kata practice
3. **Automatic**: Chunked patterns (unconscious) — real-world application

Each stage requires different pedagogy. Tutorials teach declarative. Kata compiles procedural. Extended practice automates.

**nmem mapping**: The declarative-to-procedural transition appears as stance shift from `think+diverge` to `act+converge` for a given skill across sessions.

### SECI Model: Knowledge Conversion (Nonaka & Takeuchi)

Four transformation modes:
1. **Socialization**: Tacit to tacit (learning together)
2. **Externalization**: Tacit to explicit (articulating into artifacts)
3. **Combination**: Explicit to explicit (integrating sources)
4. **Internalization**: Explicit to tacit (practicing until automatic)

Dojo crystallization is externalization. Practice from artifacts is internalization. The full cycle: session observations (tacit) -> session summaries (begin externalization) -> skill artifacts (complete externalization) -> practice from artifacts (internalization).

### Spaced Repetition and the Forgetting Curve

Without review, ~50% forgotten within an hour (Ebbinghaus). Spaced repetition reviews right before forgetting.

Algorithms:
- **SM-2**: Interval-based with ease factor
- **FSRS**: Modern, estimates retrievability/stability/difficulty directly. Best for procedural skills.

Skill decay data: d = -0.01 after training, d = -1.4 after 365+ days. Novices on complex tasks decay fastest. Overlearning is the primary mitigation.

**nmem mapping**: `learner_skills.last_practiced` and `next_review` timestamps. Review scheduled via `queue_task`. Decay detected by comparing `friction_count` in review sessions vs learning sessions.

### Error-Driven Learning

Errorful learning with corrective feedback outperforms errorless practice. Hypercorrection effect: high-confidence errors corrected more readily.

Requirements: explanatory feedback (not just "wrong"), positive error climate, explicit reflection.

**nmem mapping**: Friction episodes are learning opportunities. Pattern `error -> hint -> reasoning -> correction -> insight` captured in observation sequences within episodes. High-friction followed by smooth resolution = productive failure = prime crystallization material.

### Meta-Learning

Learning how to learn. Metacognition, self-regulated learning, cross-domain generalization.

**nmem mapping**: `s3_learn.rs` cross-session patterns are meta-learning artifacts. Aggregate analysis of `session_skills` across skills reveals which strategies work for this learner. Session summaries with `learned` fields capture effective approaches.

## Architecture: Four-Layer Design

```
Layer 4: Feedback
  failure detection, time-to-mastery, artifact versioning
      ^  performance data
Layer 3: Crystallization
  pattern synthesis, externalization, competency ontology
      ^  observations, episodes, summaries
Layer 2: Memory
  observation capture, episode detection, summarization, 5D stance
      ^  hook events (SessionStart, PostToolUse, Stop)
Layer 1: Practice
  structured sessions, scaffolding, feedback, after-action review
```

### Layer 1: Practice

Four session modalities:

**Tutorial** (Novice -> Competent): AI-led explanation, line-by-line decomposition, guided practice, independent practice, AAR. Marker: `tutorial:concept_name`. Observations: `file_read`, `file_edit`, `command`.

**Kata** (Competent -> Proficient): Timed problem, independent solve, AI review against expert patterns, reflection. Marker: `kata:kata_name:attempt_N`. Track time-to-solution, error count, solution diff vs prior attempts in `session_skills`.

**Challenge** (Proficient -> Expert): Ambiguous requirements, design phase, implementation, code review. Minimal scaffolding. Marker: `challenge:problem_name`. Observations capture design artifacts.

**Teaching** (Expert verification): Learner writes skill artifact. Another learner follows it. Gaps surface. Ultimate mastery test. Generates `skill_feedback` data.

**AI agent modes**:

| Mode | When | Behavior |
|------|------|----------|
| Instructor | New concept | Explains, demonstrates |
| Coach | Practicing | Observes, hints, doesn't solve |
| Scaffolder | Struggling | Temporary support, fading |
| Challenger | Proficient | Novel problems |
| Examiner | Assessment | Tests, scores |
| Student | Teaching mode | Plays novice, asks learner to explain |

Mode selection driven by `learner_skills.p_mastery` and current session `friction_count`.

### Layer 2: Memory

Existing nmem infrastructure — no new code needed:
- `observations` table: tool uses classified on 5 dimensions
- `work_units` table: episodes with narrative, `phase_signature`, `hot_files`, `obs_trace`
- `sessions` table: summaries with intent, learned, next_steps
- Markers: `tutorial:` prefix with concepts, range, next target
- Stance: phase/scope/locus/novelty per observation, friction per episode

Learning trajectory: `think+diverge+novel+friction` (novice) -> `act+converge+routine+smooth` (expert).

### Layer 3: Crystallization

Externalization phase of SECI model.

**Triggers** (detectable from nmem data):
- 3+ sessions on skill where `scaffolding_needed` absent from markers
- Stance shift: `novel+friction` -> `routine+smooth` for target skill
- P(mastery) >= 0.5

**Process**:
1. **Aggregate**: `SELECT * FROM sessions WHERE session_id IN (SELECT session_id FROM session_skills WHERE skill_id = ?)`
2. **Extract**: Friction locations, breakthrough moments (friction -> smooth transitions), effective explanations from episode narratives
3. **Synthesize**: Concepts concrete-to-abstract, pitfalls with remediation, actual code from sessions
4. **Validate**: Second learner attempts skill from artifact alone. Track where they struggle via `skill_feedback`.

**Teachable moment types** (extractable from `obs_trace` and episode narratives):
- **Breakthroughs**: Friction -> smooth transition within episode
- **Productive failures**: `{fail: true}` entries followed by successful resolution
- **Transfer**: `novel+smooth` stance = skill applied in new context

### Layer 4: Feedback

Four loops at different timescales:

**Loop 1 — Session**: Immediate error correction. Post-session summary captures learned/next_steps.

**Loop 2 — Concept mastery**: P(mastery) tracked per concept across sessions in `session_skills`. Scaffolding adjusted via BKT thresholds.

**Loop 3 — Artifact validation**: Aggregate `skill_feedback` across learners. Common struggle points trigger artifact revision.

**Loop 4 — Meta-learning**: Compare time-to-mastery and strategy effectiveness across skills. Apply successful patterns to new domains.

## Skill Artifact Structure

Markdown format, version-controlled, generated from nmem data.

```markdown
# Skill: [Name]

## Metadata
- Domain: [Rust | System Design | Debugging]
- Level: [Novice | Advanced Beginner | Competent | Proficient | Expert]
- Prerequisites: [skill_ids]
- Estimated sessions to mastery: [N]

## Concept
[Concise definition]

## Purpose
[Problems solved, when to use]

## Competency Gates
- [ ] Explain without notes
- [ ] Demonstrate in simple scenario
- [ ] Apply in unfamiliar context
- [ ] Identify when NOT to use
- [ ] Teach to another learner

## Core Content
[Organized by Bloom level: Remember -> Understand -> Apply]
[Worked examples with annotations]

## Common Pitfalls
- [What goes wrong] -> [How to fix]

## Practice Exercises
1. (Novice): [description, scaffolding, success criteria]
2. (Advanced Beginner): [less scaffolding]
3. (Competent): [minimal scaffolding, real-world]

## Review Schedule
[FSRS-derived intervals]

## Session History
[Auto-populated from nmem: dates, friction, breakthroughs]

## Metacognitive Notes
[Strategies that worked for this skill]
```

## Competence Progression Tracking

### Bayesian Knowledge Tracing

BKT parameters:
- P(L0): initial mastery probability
- P(T): learning transition probability
- P(S): slip (knows but errors)
- P(G): guess (doesn't know but correct)

Update after each attempt:
```
P(mastery | success) = 0.95 * P / (0.95 * P + 0.3 * (1 - P))
P(mastery | failure) = 0.05 * P / (0.05 * P + 0.7 * (1 - P))
```
Cap at 0.99 to allow decay.

Mastery thresholds -> Dreyfus stage:
- P >= 0.9: Expert (if time < threshold)
- P >= 0.7: Proficient
- P >= 0.5: Competent
- P >= 0.3: Advanced Beginner
- P < 0.3: Novice

DKT (LSTM-based) achieves AUC 0.85 vs BKT's 0.68. Start with BKT; graduate when data volume justifies.

**nmem mapping**: P(mastery) stored in `session_skills.mastery_estimate` (per session) and `learner_skills.p_mastery` (current). Updated at session end by querying `work_units` friction counts for skill-tagged episodes.

### Adaptive Scaffolding

| P(mastery) | Scaffolding | Support |
|------------|-------------|---------|
| < 0.3 | Heavy | Worked examples, step-by-step, frequent hints |
| 0.3 - 0.7 | Moderate | Guiding questions, partial solutions |
| 0.7 - 0.9 | Light | Verification only, challenge problems |
| >= 0.9 | None | Open-ended design, teaching mode |

Triggers:
- 3+ failures same concept: increase scaffolding
- 3+ rapid successes: decrease scaffolding, add variation
- No progress >10 min: agent intervenes

Expertise reversal detection: learner explains concepts before AI -> scaffolding is counterproductive -> reduce.

**nmem mapping**: Scaffolding recommendation injected at `SessionStart` via `s4_context.rs`. Input: `learner_skills.p_mastery` for target skill.

### Spaced Repetition Scheduler

Integration via `queue_task`:

Intervals (performance-based expansion):
- Good: 1d -> 3d -> 7d -> 14d -> 30d
- Hard: contract to 1-2d
- Retrievability < 80%: proactive reminder

FSRS preferred over SM-2 for procedural skills — directly estimates retrievability, stability, difficulty per learner.

Review activities (varied, not just recall):
- Retrieval practice: explain from memory
- Application: solve new problem
- Teaching: explain to AI playing novice

**nmem mapping**: `learner_skills.next_review` timestamp. Scheduler queries overdue reviews, creates tasks via `queue_task`. Review session tagged with `kata:skill:review` marker.

### Skill Trees

Skills form a DAG. Prerequisites enforced via `skills.prerequisites` (JSON array of skill_ids).

```
rust-basics -> rust-ownership -> rust-lifetimes -> rust-async
                              -> rust-borrowing -> rust-closures
```

When learner struggles with advanced skill (high friction, P(mastery) not increasing), re-check prerequisites via `learner_skills.p_mastery` for dependency skills.

## Design Patterns

### Backward Design (Wiggins & McTighe)

1. Define competency: "Learner can [outcome]"
2. Define assessment: "Demonstrated by [task]"
3. Design practice: "Built through [exercises]"

Apply when creating skill artifacts. Assessment criteria derive from competency definition, not the other way around.

### Declarative -> Procedural -> Automatic Pipeline

1. **Declarative** (Tutorial): Conscious rules. `think+diverge` stance.
2. **Procedural** (Kata): Compiled productions. `act+converge` stance.
3. **Automatic** (Real-world): Chunked patterns. `act+converge+routine+smooth` stance.

Each stage needs different pedagogy. Don't skip stages.

**nmem mapping**: Pipeline progress visible in stance trajectory per skill across sessions. Query:
```sql
SELECT s.started_at, sk.mastery_estimate,
       COUNT(*) FILTER(WHERE o.phase = 'think') as think_pct,
       COUNT(*) FILTER(WHERE o.phase = 'act') as act_pct
FROM sessions s
JOIN session_skills sk ON s.session_id = sk.session_id
JOIN observations o ON o.session_id = s.session_id
WHERE sk.skill_id = ?
GROUP BY s.session_id ORDER BY s.started_at;
```

### Worked Example Fading

1. Full worked example (Novice): complete solution with annotations
2. Partial example (Advanced Beginner): gaps for learner to fill
3. Problem only (Competent): learner solves independently
4. Ambiguous challenge (Proficient): learner defines problem and solution

**nmem mapping**: Fading tracked via marker `scaffolding_needed` field across sessions for same concept. Absent = fading complete.

### Tutorial Loop Enhancement

Existing pattern: user picks function, agent decomposes line-by-line, marker created.

Enhanced:

**Before**: `file_history` and `search` for prior markers on target skill. Check `learner_skills.next_review` for overdue reviews. Set learning objective.

**During**:
- Read phase (`think+diverge`): decompose code, activate prior knowledge
- Practice phase (`act+converge`): learner attempts, agent coaches, retrieval checkpoints
- Reflection phase (`think+converge`): learner articulates, agent probes

**After**: Marker with skills/level/assessment. Update `session_skills`. Schedule review via `queue_task`. Flag breakthroughs for crystallization.

### After-Action Review

From simulation training. Structured reflection at session end:
1. What was planned? (session intent from `sessions.summary`)
2. What happened? (episode narrative from `work_units`)
3. Why? (friction analysis from `obs_trace`)
4. What next? (next_steps, adjusted approach)

## Integration with nmem

The Dojo layers on top of nmem. No modifications to core modules.

**nmem provides**: Session management, observation capture, episode detection/narrative, 5D stance, summaries/markers, secret filtering, retention, task queue.

**Dojo adds**: Skill taxonomy, progression tracking, crystallization engine, spaced repetition, performance analytics.

### Schema Extensions

Three tables added to `schema.rs` migrations:

```sql
CREATE TABLE skills (
    skill_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    domain TEXT NOT NULL,
    prerequisites TEXT,       -- JSON array of skill_ids
    artifact_path TEXT,       -- path to skill markdown
    difficulty REAL,
    mastery_criteria TEXT,    -- JSON: {gates: [...]}
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE session_skills (
    session_id TEXT,
    skill_id TEXT,
    mastery_estimate REAL,    -- P(mastery) after this session
    friction_count INTEGER,
    breakthrough BOOLEAN,
    PRIMARY KEY (session_id, skill_id),
    FOREIGN KEY (session_id) REFERENCES sessions(session_id),
    FOREIGN KEY (skill_id) REFERENCES skills(skill_id)
);

CREATE TABLE learner_skills (
    learner_id TEXT DEFAULT 'default',
    skill_id TEXT,
    current_level TEXT,       -- Dreyfus stage
    p_mastery REAL,           -- BKT estimate [0,1]
    last_practiced INTEGER,   -- Unix timestamp
    next_review INTEGER,      -- FSRS-scheduled
    exercises_attempted INTEGER DEFAULT 0,
    exercises_mastered INTEGER DEFAULT 0,
    PRIMARY KEY (learner_id, skill_id),
    FOREIGN KEY (skill_id) REFERENCES skills(skill_id)
);
```

### Extension Points

1. **Markers**: `tutorial:skill:concept` convention (already in use)
2. **Episode annotations**: Skills practiced, level targeted, breakthroughs — stored in `work_units` metadata
3. **Session summaries**: Extend `s1_4_summarize.rs` output with `skills_practiced`, `level_progress`
4. **Context injection**: Extend `s4_context.rs` SessionStart to include skill/mastery/scaffolding when dojo tables populated
5. **Observation tagging**: Content field includes `skill:rust:ownership` for FTS5 filtering

### MCP Tool Extensions

- `queue_dojo_session`: Schedule practice for specific skill
- `get_competency_matrix`: Current mastery state across all skills
- `suggest_next_practice`: Recommend skill/exercise from mastery + review schedule + prerequisites
- `crystallize_skill`: Generate skill artifact from session history

### CLI Extensions

- `nmem dojo status`: Skill tree with mastery levels
- `nmem dojo review`: Skills needing spaced repetition review
- `nmem dojo crystallize <skill>`: Generate artifact from nmem data

## Pitfalls and Failure Modes

### Practice
- **Comfort zone**: Repeating mastered material. Detect: `routine+smooth` on exercises that should challenge. Fix: increase difficulty.
- **Insufficient specificity**: "Practice Rust" vs "practice ownership transfer". Sessions must target specific competencies.
- **No feedback**: Practicing errors uncorrected. Immediate corrective feedback is non-negotiable.
- **Burnout**: Deliberate practice is effortful. Monitor session frequency via `sessions` table.

### Assessment
- **Teaching to test**: Narrow gates miss broader understanding. Use behavioral + conceptual + transfer evidence.
- **False mastery**: Passing once is not retention. Spaced repetition required.
- **Expertise reversal**: Novice scaffolding harms experts. Adapt to `learner_skills.p_mastery`.
- **Example overfitting**: Memorizing worked examples without principles. Include novel transfer problems.

### Cognitive Load
- **Extraneous overload**: Bad design, not hard material. Detect: friction on routine-classified observations.
- **Intrinsic overload**: Complex skills before prerequisites. Enforce mastery gates via `skills.prerequisites`.
- **Premature complexity**: Novices without schemas. Chunk and build incrementally.

### Crystallization
- **Expert blind spots**: Author assumes learner knowledge. Detect via `skill_feedback` struggle clustering: "80% stuck at Step 3".
- **Over-abstraction**: Loses practical applicability. Include concrete examples.
- **Stale artifacts**: Not updated from feedback. Living documents with version control.
- **Context collapse**: Too much context removed for transfer.

### Feedback
- **Delayed**: Corrections too late to connect error with action.
- **Overload**: Too many corrections at once.
- **No action loop**: Feedback without actionable next step.
- **Vanity metrics**: Completion rates instead of competency. Track P(mastery), not sessions attended.

### System
- **Premature optimization**: DKT before BKT validates. Start simple.
- **Gamification backfire**: External rewards decrease intrinsic motivation. Progression markers for information only.
- **Dashboard overload**: Too much data. Surface only actionable metrics.
- **Ignoring affect**: Cognitive-only focus. Monitor session abandonment (sessions without `Stop` hook).
- **Fragile prerequisites**: Learner bypasses with low mastery. Re-check when advanced skill friction appears.
- **One-size pedagogy**: Different learners, different strategies. Track via meta-learning.

## Implementation Roadmap

### Phase 1: Foundation (MVP)

Existing nmem provides memory layer. Add:
1. `skills`, `session_skills`, `learner_skills` tables (schema migration)
2. Simple BKT: update `p_mastery` after each skill-tagged session
3. Skill artifact template (markdown)
4. `nmem dojo status` CLI
5. Convert existing Rust tutorial markers into first skill entry

### Phase 2: Scaffolding

1. Rules engine: P(mastery) thresholds -> scaffolding level
2. Context injection: skill/mastery/scaffolding at SessionStart
3. Exercise library: kata files per skill
4. Worked example DB from prior sessions

### Phase 3: Spaced Repetition

1. Review scheduler: `queue_task` integration
2. FSRS for interval computation
3. Retention tracking: review vs learning performance
4. Decay detection and proactive reminders

### Phase 4: Crystallization Pipeline

1. Automated artifact generation from nmem queries
2. Pattern extraction: breakthroughs, failures, effective explanations
3. Versioning and validation with second learner
4. Feedback loop: `skill_feedback` -> artifact updates

### Phase 5: Meta-Learning

1. Strategy effectiveness tracking per learner
2. Cross-skill transfer detection
3. Learning velocity analytics
4. Skill tree visualization

## Potential ADRs

**ADR: Skill Artifact Format and Lifecycle**
Question: Structure, versioning, crystallization trigger?
Evidence: 6/6 researchers converge on Markdown, SECI externalization, semver. Crystallization at 3+ sessions with P(mastery) >= 0.5. Living document pattern.

**ADR: Knowledge Tracing Selection**
Question: BKT vs DKT vs FSRS?
Evidence: BKT sufficient at small scale (AUC 0.68). FSRS for scheduling (modern, maintained). DKT (AUC 0.85) when data volume justifies. Recommendation: BKT for mastery, FSRS for scheduling.

**ADR: Scaffolding Adaptation**
Question: How to adjust difficulty and support?
Evidence: Expertise reversal (Kalyuga) makes fixed scaffolding harmful. Adaptive fading via P(mastery) thresholds. Stance signals (friction/novelty) as real-time input.

**ADR: Dojo Schema**
Question: What tables to add?
Evidence: Minimal set: `skills`, `session_skills`, `learner_skills`. Foreign keys to existing `sessions`. No core table modifications.

**ADR: Spaced Repetition Integration**
Question: How to schedule reviews?
Evidence: FSRS algorithm via `queue_task`. Skill decay d = -1.4 at 365 days. Review activities: retrieval, application, teaching.

## Potential Skills

Claude Code `/skills` that this design produces.

**`/dojo-start`**
Trigger: User wants to practice a skill.
Action: Query `learner_skills` for target skill. Check `next_review` for overdue items. Set session type based on `p_mastery` (tutorial if < 0.3, kata if 0.3-0.7, challenge if > 0.7). Inject scaffolding recommendation. Create session marker on completion with skill/level/assessment. Update `session_skills`.

**`/dojo-review`**
Trigger: Scheduled review or user request.
Action: Query `learner_skills WHERE next_review < now()`. Select review activity. Track performance vs prior. Update `p_mastery` and `next_review`.

**`/dojo-crystallize`**
Trigger: Skill reaches P(mastery) >= 0.5 with 3+ sessions.
Action: Query all session data for skill. Extract patterns from episode narratives and `obs_trace`. Generate markdown artifact. Write to `skills.artifact_path`.

**`/dojo-status`**
Trigger: User wants progress view.
Action: Query `learner_skills` joined with `skills`. Show DAG with mastery levels, overdue reviews, recommended next skill.

## Kaizen Metrics

All derivable from nmem's data model. No external instrumentation needed.

### Friction Rate per Skill
```sql
SELECT sk.skill_id, sk.name,
    ROUND(AVG(CASE WHEN ss.friction_count > 0 THEN 1.0 ELSE 0.0 END), 2) as friction_rate
FROM session_skills ssk
JOIN skills sk ON ssk.skill_id = sk.skill_id
GROUP BY sk.skill_id;
```
Decreasing over sessions = learning. Flat = plateau.

### Time-to-Mastery
```sql
SELECT skill_id,
    MIN(s.started_at) as first_session,
    MIN(CASE WHEN ssk.mastery_estimate >= 0.9 THEN s.started_at END) as mastery_session,
    COUNT(DISTINCT s.session_id) as sessions_to_mastery
FROM session_skills ssk JOIN sessions s ON ssk.session_id = s.session_id
GROUP BY skill_id;
```
Shorter on later skills = meta-learning working.

### Skill Reuse
How often crystallized skills appear in non-dojo sessions. High reuse = transfer to real work.
```sql
SELECT skill_id, COUNT(*) as uses
FROM observations o
JOIN sessions s ON o.session_id = s.session_id
WHERE o.content LIKE '%skill:rust:ownership%'
  AND s.session_id NOT IN (SELECT session_id FROM session_skills)
GROUP BY skill_id;
```

### Error Pattern Reduction
Frequency of specific error types per skill over time. Declining = skill internalized.
```sql
SELECT strftime('%Y-%W', o.created_at, 'unixepoch') as week,
    COUNT(*) FILTER(WHERE o.content LIKE '%borrow checker%') as borrow_errors
FROM observations o
JOIN session_skills ssk ON o.session_id = ssk.session_id
WHERE ssk.skill_id = 'rust-ownership' AND o.obs_type = 'command'
GROUP BY week ORDER BY week;
```

### Stance Trajectory Convergence
Sessions until stance shifts from `think+diverge` to `act+converge` for a skill.
```sql
SELECT ssk.skill_id, s.started_at,
    ROUND(AVG(CASE WHEN o.phase = 'act' AND o.scope = 'converge' THEN 1.0 ELSE 0.0 END), 2) as act_converge_ratio
FROM observations o
JOIN sessions s ON o.session_id = s.session_id
JOIN session_skills ssk ON s.session_id = ssk.session_id
WHERE o.phase IS NOT NULL
GROUP BY ssk.skill_id, s.session_id
ORDER BY ssk.skill_id, s.started_at;
```
Faster convergence on later skills = transfer learning.

### Competence Progression Curves
P(mastery) time series per skill.
```sql
SELECT ssk.skill_id, s.started_at, ssk.mastery_estimate
FROM session_skills ssk JOIN sessions s ON ssk.session_id = s.session_id
ORDER BY ssk.skill_id, s.started_at;
```
S-curve = healthy. Linear = gaming. Plateau = intervention needed. Decay = spaced repetition interval too long.

### Review Effectiveness
Performance on review vs initial sessions.
```sql
SELECT ssk.skill_id,
    AVG(CASE WHEN m.text LIKE '%review%' THEN ssk.friction_count END) as review_friction,
    AVG(CASE WHEN m.text NOT LIKE '%review%' THEN ssk.friction_count END) as learn_friction
FROM session_skills ssk
LEFT JOIN observations m ON m.session_id = ssk.session_id AND m.obs_type = 'marker'
GROUP BY ssk.skill_id;
```
Review friction <= learn friction = spaced repetition working.

### Crystallization Quality
Time-to-mastery for second learner (with artifact) vs first (without).
```sql
SELECT skill_id, learner_id,
    COUNT(DISTINCT session_id) as sessions_to_mastery
FROM session_skills
WHERE mastery_estimate >= 0.9
GROUP BY skill_id, learner_id;
-- Compare across learner_ids for same skill_id
```
Ratio < 1.0 = artifact accelerates learning.
