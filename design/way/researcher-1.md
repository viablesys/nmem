# Dojo: AI-Assisted Skill Acquisition and Crystallization System

## Executive Summary

A **Dojo** is a multi-layered training construct for AI-assisted skill acquisition that transforms learning sessions into reusable skill artifacts. Inspired by The Matrix's training environment, it operates on four recursive layers:

1. **Practice Layer** — Human learner + AI agent engage in structured sessions
2. **Memory Layer** — Cross-session memory captures progression via observations, episodes, and stance data
3. **Crystallization Layer** — Learning compresses into teachable skill artifacts (tacit→explicit knowledge)
4. **Feedback Layer** — Skill usage feeds performance data back into the Dojo, improving the artifact

The system bridges cognitive science (deliberate practice, zone of proximal development, schema theory) with AI capabilities (memory continuity, pattern detection, adaptive scaffolding) to create a self-improving learning environment.

**Target context:** An AI coding agent with cross-session memory (nmem) that already tracks tutorial sessions via markers. The Dojo formalizes this into a domain-agnostic system: "I want to learn X" → "here's a battle-tested skill for X."

---

## 1. Theoretical Foundation

### 1.1 Skill Acquisition Frameworks

#### Dreyfus Model: Five Stages of Competency

The [Dreyfus Model](https://en.wikipedia.org/wiki/Dreyfus_model_of_skill_acquisition) (Dreyfus & Dreyfus, 1980) defines skill progression through five stages:

| Stage | Characteristics | Dojo Mapping |
|-------|----------------|--------------|
| **Novice** | Context-free rules, step-by-step, slow/clumsy, no judgment | Initial tutorial sessions, high scaffolding, explicit rule presentation |
| **Advanced Beginner** | Situational rules, patterns emerge, limited context | Repeated practice with variation, pattern recognition exercises |
| **Competent** | Independent decision-making, handles complexity, goal-driven | Multi-session projects, debugging challenges, less scaffolding |
| **Proficient** | Intuitive decision-making, sees situations holistically, uses maxims | Cross-domain application, performance optimization, teaches others |
| **Expert** | Unconscious competence, fluid performance, no deliberation | Skill crystallization, can critique and extend the domain |

**Key insight:** Progression is non-linear. The Dojo must detect the learner's current stage and adjust instruction accordingly (see [Adaptive Scaffolding](#42-adaptive-scaffolding)).

**Research evidence:** [Expertise develops through deliberate practice](https://pubmed.ncbi.nlm.nih.gov/18778378/) (Ericsson, 2008), not just experience. The Dojo must enforce deliberate, goal-directed practice with feedback.

#### Bloom's Taxonomy: Cognitive Depth

[Bloom's Taxonomy](https://en.wikipedia.org/wiki/Bloom's_taxonomy) (revised 2001) structures learning objectives from lower-order to higher-order cognitive skills:

1. **Remember** — Recall facts, concepts (e.g., "What is a lifetime in Rust?")
2. **Understand** — Explain ideas (e.g., "Why does Rust require explicit lifetimes?")
3. **Apply** — Use knowledge in new situations (e.g., "Fix this borrow checker error")
4. **Analyze** — Break down concepts (e.g., "Compare stack vs heap allocation strategies")
5. **Evaluate** — Make judgments (e.g., "Critique this API design for memory safety")
6. **Create** — Produce original work (e.g., "Design a zero-copy parser")

**Dojo application:** Each skill level targets specific cognitive levels. Novice sessions focus on Remember/Understand, while Expert sessions target Evaluate/Create. The memory layer tracks which levels the learner has demonstrated ([competency progression tracking](#44-competency-progression-tracking)).

**Sources:** [Bloom's Taxonomy of Educational Objectives](https://teaching.uic.edu/cate-teaching-guides/syllabus-course-design/blooms-taxonomy-of-educational-objectives/), [Using Bloom's Taxonomy](https://uwaterloo.ca/centre-for-teaching-excellence/catalogs/tip-sheets/blooms-taxonomy)

#### Zone of Proximal Development (ZPD)

[Vygotsky's ZPD](https://www.simplypsychology.org/zone-of-proximal-development.html) defines the sweet spot between "too easy" (independent capability) and "too hard" (beyond reach even with help). Learning occurs most effectively in the ZPD with **scaffolding** — temporary support that's gradually withdrawn.

**Effect size:** Contingent scaffolding (adjusted to the learner's ZPD) is **2.5× more effective** than fixed support ([Van de Pol et al., 2010](https://educationaltechnology.net/vygotskys-zone-of-proximal-development-and-scaffolding/)). Hattie (2009) reports scaffolding effect size of **0.82** (strong).

**Dojo implementation:**
- **ZPD detection:** The memory layer's stance data (5 dimensions) reveals when the learner enters friction (failures > 0). High friction + low progress = outside ZPD. The Dojo adjusts difficulty.
- **Gradual release:** I Do → We Do → You Do model. Early sessions: agent demonstrates, learner observes. Middle: collaborative problem-solving. Late: learner works independently, agent reviews.

**Sources:** [Zone of Proximal Development](https://www.simplypsychology.org/zone-of-proximal-development.html), [Vygotsky's Zone of Proximal Development and Scaffolding](https://educationaltechnology.net/vygotskys-zone-of-proximal-development-and-scaffolding/)

### 1.2 Deliberate Practice Theory

[Ericsson's deliberate practice](https://pubmed.ncbi.nlm.nih.gov/18778378/) framework defines expert performance acquisition:

1. **Well-defined goals** — Each session targets specific sub-skills (not vague "learn Rust")
2. **Immediate feedback** — The AI agent corrects misconceptions in real-time
3. **Repetition with variation** — Same concept, different contexts (e.g., lifetimes in structs, functions, traits)
4. **Mental representation refinement** — Schema building (see [1.5](#15-schema-theory-and-chunking))
5. **Sustained effort** — Sessions push just beyond current capability (ZPD alignment)

**Critical components:**
- **Mentor guidance:** The AI agent serves as master teacher, identifying goals and methods for practice
- **Solitary practice:** Between sessions, the learner practices independently; next session reviews
- **Focus on weaknesses:** The Dojo tracks per-concept competency and targets gaps

**Pitfall:** "The 10,000-hour rule" is a misinterpretation. [Recent research](https://pmc.ncbi.nlm.nih.gov/articles/PMC7461852/) shows deliberate practice explains **only ~26% of variance** in performance (domain-dependent). The Dojo must combine practice with feedback, mental models, and spaced repetition.

**Sources:** [Deliberate Practice and Acquisition of Expert Performance](https://pubmed.ncbi.nlm.nih.gov/18778378/), [Is the Deliberate Practice View Defensible?](https://pmc.ncbi.nlm.nih.gov/articles/PMC7461852/)

### 1.3 Spaced Repetition and Memory Consolidation

[Spaced repetition systems (SRS)](https://traverse.link/spaced-repetition/srs-spaced-repetition-system) leverage the **spacing effect**: information reviewed at increasing intervals moves from short-term to long-term memory more effectively than massed practice (cramming).

**Research evidence:**
- Meta-analysis of 29 studies: spaced practice **74% more effective** than massed practice
- Can improve long-term retention by **up to 200%**
- [Memory consolidation during sleep](https://pmc.ncbi.nlm.nih.gov/articles/PMC5476736/) is influenced by spaced learning during waking hours

**SRS algorithms:** Evolution from fixed-interval (Leitner System) to adaptive (SSP-MMC, LSTM-HLR) that personalize review schedules based on learner performance.

**Dojo integration:**
- **Concept tagging:** Each tutorial session tags concepts covered (e.g., `lifetimes`, `borrow_checker`, `trait_bounds`)
- **Spaced review triggers:** After 1 day, 3 days, 1 week, 2 weeks, 1 month, query: "Explain [concept] without looking at code"
- **Retrieval practice:** Active recall (generate answer) beats passive review. The Dojo quizzes, doesn't just re-present.

**Sources:** [Spaced Repetition and Retrieval Practice](https://journals.zeuspress.org/index.php/IJASSR/article/view/425), [Spacing Repetitions Over Long Timescales](https://pmc.ncbi.nlm.nih.gov/articles/PMC5476736/)

### 1.4 Situated Learning and Cognitive Apprenticeship

[Lave & Wenger's situated learning](https://infed.org/dir/welcome/jean-lave-etienne-wenger-and-communities-of-practice/) (1991) argues learning is inherently social, situated in authentic contexts through **legitimate peripheral participation (LPP)** — newcomers start at the periphery of a community of practice, gradually moving to full participation.

**Cognitive apprenticeship** ([Brown, Collins & Duguid, 1989](https://martialarts.fandom.com/wiki/Ranking_system)) makes thinking visible:
1. **Modeling** — Expert demonstrates while verbalizing thought process
2. **Coaching** — Expert observes learner, provides hints/feedback
3. **Scaffolding** — Temporary support structures
4. **Articulation** — Learner explains their reasoning
5. **Reflection** — Compare learner's process to expert's
6. **Exploration** — Learner tackles novel problems independently

**Dojo application:**
- **Modeling:** Early Rust tutorial sessions — agent reads code line-by-line, explains "why Rust requires this"
- **Coaching:** Middle sessions — learner writes code, agent watches, intervenes at errors
- **Articulation:** "Explain why you chose `&mut` here" — forces metacognition
- **Reflection:** After-action review (see [1.6](#16-simulation-training-and-after-action-reviews)) compares learner's solution to expert patterns

**Sources:** [Jean Lave, Etienne Wenger and communities of practice](https://infed.org/dir/welcome/jean-lave-etienne-wenger-and-communities-of-practice/), [Situated Learning Theory](https://opentext.wsu.edu/theoreticalmodelsforteachingandresearch/chapter/situated-learning-theory/)

### 1.5 Schema Theory and Chunking

[Cognitive Load Theory](https://www.structural-learning.com/post/cognitive-load-theory-a-teachers-guide) (Sweller, 1988) explains how working memory limitations constrain learning. Expertise develops through:

1. **Chunking** — Combining elements into higher-order schemas (e.g., "ownership pattern" = borrow + lifetime + drop)
2. **Schema automation** — Repeated practice moves schemas from conscious (working memory) to unconscious (long-term memory)
3. **Reduced cognitive load** — Automated schemas free working memory for higher-level reasoning

**Key insight:** A single schema can combine many information pieces, processed as one element by working memory. [Experts possess more complex schemas](https://link.springer.com/article/10.1007/s10648-024-09848-3) built from lower-level schemas.

**Dojo implications:**
- **Progressive complexity:** Early sessions present small chunks (single concept). Later sessions combine chunks (ownership + traits + lifetimes).
- **Automation signals:** Track time-to-solution and error rate. Decreasing values indicate automation. The Dojo advances when schemas are automated.
- **Expertise reversal effect:** Instructional methods for novices can **harm** experts. The Dojo adapts scaffolding to skill level.

**Sources:** [Cognitive Load Theory](https://www.structural-learning.com/post/cognitive-load-theory-a-teachers-guide), [Chunking as a pedagogy](https://paulgmoss.com/2023/02/13/chunking-as-a-pedagogy/)

### 1.6 Simulation Training and After-Action Reviews

[Flight simulator research](https://commons.erau.edu/ijaaa/vol5/iss1/6/) reveals critical insights for training transfer:

- **Fidelity paradox:** Physical fidelity (realistic visuals/controls) matters less than **functional fidelity** — the presence of retrieval cues that activate real-world mental models
- **Instructor role underestimated:** [Very little research](https://commons.erau.edu/ijaaa/vol5/iss1/6/) on instructor interaction, yet it's critical for preventing negative transfer
- **Motion fidelity modest effect:** Realistic motion helps, but effect size is small; feedback matters more

**After-Action Review (AAR)** — structured debriefing after simulated experience:
- **Effect size:** Debriefs improve performance **~25%** (d = 0.67) over control groups ([meta-analysis, 46 samples](https://www.researchgate.net/publication/236068923_Do_Team_and_Individual_Debriefs_Enhance_Performance_A_Meta-Analysis))
- **Phases:** Reaction → Analysis → Summary (RAS model, Rudolph et al.)
- **Critical mechanism:** [Facilitator-guided reflection](https://www.ncbi.nlm.nih.gov/books/NBK546660/) helps identify knowledge/skill gaps

**Dojo application:**
- **Session-end AAR:** Every tutorial session closes with structured review:
  1. **What happened?** (observation recap)
  2. **What was supposed to happen?** (compare to goal/standard)
  3. **Why the gap?** (identify misconceptions, missing schemas)
  4. **How to close it?** (next session's focus)
- **Agent as facilitator:** AI guides reflection without giving away answers ("What would happen if you moved that `&mut` to the function signature?")

**Sources:** [Flight Simulator Fidelity, Training Transfer](https://commons.erau.edu/ijaaa/vol5/iss1/6/), [Do Team and Individual Debriefs Enhance Performance?](https://www.researchgate.net/publication/236068923_Do_Team_and_Individual_Debriefs_Enhance_Performance_A_Meta-Analysis), [Debriefing Techniques in Medical Simulation](https://www.ncbi.nlm.nih.gov/books/NBK546660/)

### 1.7 Mastery Learning and Competency-Based Education

[Mastery learning](https://pmc.ncbi.nlm.nih.gov/articles/PMC10159400/) (Bloom, 1968) requires learners reach a **threshold level** before advancing. Key principles:

1. **Criterion-referenced assessment** — Students judged against a standard, not peers
2. **Formative feedback loops** — Frequent low-stakes assessments identify gaps
3. **Corrective instruction** — Students who don't meet threshold receive targeted help and retest
4. **Flexible pacing** — Time is variable, mastery is fixed (opposite of traditional education)

**Research evidence:**
- **+5 months progress** on average ([Education Endowment Foundation](https://www.tandfonline.com/doi/full/10.1080/03075079.2023.2217203))
- **Effect size 0.59**, higher with stricter mastery thresholds ([A Practical Review of Mastery Learning](https://pmc.ncbi.nlm.nih.gov/articles/PMC10159400/))
- **Formative assessment must drive corrective instruction** for full effect

**Dojo implementation:**
- **Mastery checkpoints:** Each skill artifact defines **success criteria** (e.g., "Can implement a custom iterator trait without errors in <30 min")
- **No progression until mastery:** The Dojo blocks advancing to "Proficient" Rust sessions until "Competent" criteria met
- **Corrective loops:** If checkpoint fails, the system generates targeted exercises addressing specific gaps (not generic "study more")

**Competency-based education** extends this: [learners proceed at own pace](https://www.tandfonline.com/doi/full/10.1080/03075079.2023.2217203), competency (not seat time) is the currency.

**Sources:** [A Practical Review of Mastery Learning](https://pmc.ncbi.nlm.nih.gov/articles/PMC10159400/), [Competency-based learning and formative assessment](https://www.tandfonline.com/doi/full/10.1080/03075079.2023.2217203)

---

## 2. Existing Training Systems: Patterns and Lessons

### 2.1 Code Kata and Programming Dojos

[Code kata](http://codekata.com/) (Dave Thomas, 2006) are small, repeatable programming exercises focused on practice, not solutions. [Programming dojos](https://codingdojo.org/kata/) are collaborative sessions where teams practice katas.

**Key patterns:**
- **Repetition with variation:** Solve the same kata (e.g., FizzBuzz, Conway's Game of Life) multiple times, focusing on different techniques (TDD, functional style, performance optimization)
- **Constraints:** Artificial limits (e.g., "no if statements," "lines <80 chars") force creative problem-solving
- **Social learning:** Pair/mob programming, code review, shared reflection
- **No stakes:** Kata are throwaway code; failure is safe

**Dojo integration:**
- **Kata as spaced practice:** After learning a Rust concept (e.g., `Option`), assign a kata that requires it (e.g., "Safe division function")
- **Progressive kata sequences:** Kata ordered by difficulty, each building on prior schemas
- **Constraint mode:** "Implement this without using `unwrap()`" — forces idiomatic patterns

**Pitfall:** Kata alone don't build expertise. [Research shows](https://dev.to/stealthmusic/why-you-should-do-coding-dojos-3n4j) they improve fluency but need integration with real-world projects for transfer.

**Sources:** [Code Kata](http://codekata.com/), [Coding Dojo](https://codingdojo.org/kata/), [Why You Should Do Coding Dojos](https://dev.to/stealthmusic/why-you-should-do-coding-dojos-3n4j)

### 2.2 Martial Arts Belt Systems

[Belt ranking systems](https://www.andrewballmma.com/belt-ranking-system) (originated in Judo, 1880s by Jigoro Kano) provide visible progression markers and structured skill development.

**Core principles:**
- **Hierarchical progression:** White (beginner) → Yellow → Orange → Green → Blue → Purple → Brown → Black
- **Explicit requirements:** Each belt has defined techniques, forms, sparring requirements
- **Time + competency gates:** Minimum training period + demonstration of mastery
- **Holistic assessment:** [Technical proficiency + intangibles](https://zenplanner.com/blogs/developing-a-comprehensive-mma-belt-progression-and-ranking-system/) (discipline, attitude, teaching ability)
- **Public recognition:** Belt color signals capability to community

**Dojo application:**
- **Skill badges:** Visible markers in the Dojo system (Novice → Advanced Beginner → Competent → Proficient → Expert)
- **Requirements transparency:** Each level lists specific capabilities (e.g., "Proficient Rust: can design async APIs with lifetimes, debug race conditions, contribute to open-source projects")
- **Demonstration, not self-assessment:** Advancement requires passing challenges, not claiming readiness
- **Teaching requirement:** To reach "Expert," must create a tutorial/skill artifact for others (knowledge crystallization)

**Pitfall:** Belt inflation (promoting too easily) degrades the system. [Brazilian jiu-jitsu features fewer belts, longer times](https://www.martialytics.com/blog/belt-system-martial-arts-your-complete-ranking-guide) to prevent this.

**Sources:** [Belt Ranking System](https://www.andrewballmma.com/belt-ranking-system), [Martial Arts Belt Levels](https://sixthsensemma.com/martial-arts-belt-levels/)

### 2.3 Flight Simulators and High-Fidelity Training

Flight simulators provide [controlled, repeatable practice](https://commons.erau.edu/ijaaa/vol5/iss1/6/) for high-stakes skills. Key design lessons:

**Fidelity dimensions:**
- **Physical fidelity:** Visual/tactile realism
- **Functional fidelity:** Behavioral realism (controls respond correctly)
- **Task fidelity:** Scenarios match real-world workflow
- **Psychological fidelity:** Stress/time pressure match reality

**Critical insight:** [Functional fidelity > physical fidelity](https://commons.erau.edu/ijaaa/vol5/iss1/6/) for transfer. A low-fi simulator with correct retrieval cues outperforms a high-fi simulator with incorrect cues.

**Instructor-in-the-loop:** Simulators alone are insufficient. [Validated flight models + skilled instructors](https://commons.erau.edu/ijaaa/vol5/iss1/6/) are necessary to judge realism and prevent negative transfer.

**Dojo application:**
- **Code environment fidelity:** Tutorial sessions use the learner's actual dev environment (not toy IDE), real compiler errors, real tools
- **Task fidelity:** Problems mirror real-world scenarios (not contrived exercises)
- **Psychological fidelity:** Timed challenges, debugging under pressure (simulates production incident response)
- **Agent as instructor:** Validates exercises, prevents shortcuts that cause negative transfer (e.g., "Always use `unwrap()`" works in tutorials, fails in production)

**Sources:** [Flight Simulator Fidelity, Training Transfer](https://commons.erau.edu/ijaaa/vol5/iss1/6/), [Simulator Fidelity Overview](https://www.sciencedirect.com/topics/engineering/simulator-fidelity)

### 2.4 Intelligent Tutoring Systems (ITS)

[AI-driven ITS](https://pmc.ncbi.nlm.nih.gov/articles/PMC12078640/) adapt to individual learners using ML/AI. State-of-the-art systems (2024-2026):

**Core capabilities:**
- **Learner modeling:** Track knowledge state, learning style, affective state (frustration, boredom)
- **Real-time adaptation:** Adjust difficulty, pacing, content based on performance
- **Immediate feedback:** Correct errors, explain misconceptions
- **Generative AI integration:** [GPT-4-based ITS](https://arxiv.org/abs/2410.10650) generate dynamic exercises, personalized hints

**Evidence:** ITS can [transform learning by adapting to individual needs](https://slejournal.springeropen.com/articles/10.1186/s40561-023-00260-y), providing real-time feedback and tailored content.

**Architecture patterns:**
- **Domain model:** Knowledge graph of concepts and dependencies
- **Student model:** Bayesian Knowledge Tracing or Deep Knowledge Tracing ([DKT](https://dl.acm.org/doi/10.1145/2330601.2330661)) to estimate mastery probabilities
- **Pedagogical model:** Select next action (hint, problem, explanation) via reinforcement learning

**Dojo integration:**
- **Concept dependency graph:** Rust lifetimes depend on ownership; the Dojo won't teach lifetimes until ownership is mastered
- **Knowledge tracing:** Model P(mastery | observations) for each concept, use to trigger reviews
- **LLM-generated hints:** When learner stuck, GPT-4 generates contextual hint (not full solution)

**Pitfall:** Over-reliance on automation. [Recent systematic review](https://pmc.ncbi.nlm.nih.gov/articles/PMC12078640/) warns ITS can reduce human interaction, which is critical for motivation and higher-order thinking.

**Sources:** [A systematic review of AI-driven ITS in K-12](https://pmc.ncbi.nlm.nih.gov/articles/PMC12078640/), [Generative AI and Its Impact on ITS](https://arxiv.org/abs/2410.10650), [AI in intelligent tutoring systems](https://slejournal.springeropen.com/articles/10.1186/s40561-023-00260-y)

---

## 3. Knowledge Crystallization: From Tacit to Explicit

### 3.1 The SECI Model (Nonaka & Takeuchi)

[The SECI model](https://en.wikipedia.org/wiki/SECI_model_of_knowledge_dimensions) describes knowledge conversion in four modes:

1. **Socialization (tacit → tacit):** Learning by observation, apprenticeship
2. **Externalization (tacit → explicit):** Articulating intuition into concepts, metaphors, models
3. **Combination (explicit → explicit):** Synthesizing documented knowledge into new forms
4. **Internalization (explicit → tacit):** Practicing documented procedures until automatic

**Externalization is the bottleneck:** [Making tacit knowledge explicit](https://bloomfire.com/blog/implicit-tacit-explicit-knowledge/) is hard but necessary for knowledge sharing and organizational learning.

**Knowledge crystallization in the Dojo:** [The Knowledge Crystallization Cycle](https://arxiv.org/html/2603.10808) (recent 2026 research) operationalizes this for AI systems:

> "Fragmented, contextual knowledge embedded in conversational interactions is progressively transformed into structured, reusable, and transferable knowledge assets."

**Process:**
1. **Fragmented knowledge:** Early Rust tutorial sessions capture isolated insights ("Oh, `&mut` means exclusive access")
2. **Conceptual clustering:** Memory layer groups related insights (all `&mut` observations)
3. **Pattern abstraction:** Cross-session analysis identifies general principles ("Exclusive access prevents data races")
4. **Artifact generation:** The Dojo synthesizes a skill document: "Rust Ownership Model: A Practitioner's Guide"

**Feedback loop:** [Externalizing tacit knowledge through dialogue](https://bloomfire.com/blog/implicit-tacit-explicit-knowledge/) creates a recursive effect — teaching the AI agent deepens the human's self-understanding.

**Sources:** [SECI model of knowledge dimensions](https://en.wikipedia.org/wiki/SECI_model_of_knowledge_dimensions), [Tacit vs Explicit Knowledge](https://bloomfire.com/blog/implicit-tacit-explicit-knowledge/), [Knowledge Crystallization Cycle](https://arxiv.org/html/2603.10808)

### 3.2 Skill Artifact Anatomy

A crystallized skill artifact is not just a tutorial — it's a **performative document** encoding:

**1. Competency ontology** ([skill taxonomy](https://gloat.com/blog/skills-ontology-framework/))
- Hierarchical structure of sub-skills (e.g., Rust = ownership + borrowing + lifetimes + traits + ...)
- Dependencies (can't learn lifetimes before ownership)
- Relationships (borrowing is a special case of ownership)

**2. Progression pathway** (Dreyfus-aligned)
- Novice exercises (rule-following)
- Competent challenges (problem-solving)
- Expert problems (open-ended design)

**3. Mental models and schemas**
- Conceptual explanations (not just syntax)
- Common misconceptions and how to resolve them
- Expert heuristics ("When to use `Rc` vs `Arc`")

**4. Spaced practice schedule**
- Initial exercises (high frequency)
- Review intervals (1d, 3d, 1w, 2w, 1mo)
- Mastery checkpoints

**5. Assessment rubrics** ([competency-based evaluation](https://cloudassess.com/blog/competency-based-assessment/))
- Observable criteria for each level
- Performance thresholds (speed, accuracy, independence)

**6. Metadata for adaptation**
- Common failure modes (from prior learners)
- Typical time-to-mastery
- Prerequisites

**Example structure (Rust Lifetimes skill):**
```toml
[skill]
name = "Rust Lifetimes"
domain = "Rust"
version = "1.2.0"
prerequisites = ["rust-ownership", "rust-borrowing"]

[progression.novice]
goal = "Understand lifetime syntax, annotate simple functions"
exercises = ["lifetime-basics-01", "lifetime-basics-02"]
mastery_criteria = { accuracy = 0.9, time_minutes = 15 }

[progression.competent]
goal = "Resolve lifetime errors in structs, implement lifetime-generic types"
exercises = ["lifetime-structs-01", "lifetime-generics-01"]
mastery_criteria = { accuracy = 0.85, time_minutes = 30 }

[spaced_practice]
initial_interval = "1d"
intervals = ["3d", "1w", "2w", "1mo"]

[mental_models]
concepts = ["Lifetime elision rules", "Borrow checker algorithm", "Lifetime variance"]
misconceptions = [
  { error = "Thinking lifetimes create copies", remedy = "Lifetimes are compile-time only" }
]
```

**Sources:** [Skills ontology framework](https://gloat.com/blog/skills-ontology-framework/), [Competency-Based Assessment](https://cloudassess.com/blog/competency-based-assessment/)

### 3.3 Artifact Versioning and Evolution

Skills evolve. The Dojo must treat artifacts as **living documents**:

**Version triggers:**
1. **Domain changes:** Rust 2024 edition adds new syntax → skill artifact updated
2. **Failure pattern detection:** 80% of learners fail exercise #5 → exercise redesigned or scaffolding added
3. **Performance drift:** Average time-to-mastery increases → difficulty recalibrated
4. **Community feedback:** Learners report misconception not covered → mental model section expanded

**Versioning schema:** SemVer-inspired:
- **Major (1.0 → 2.0):** Incompatible changes (Rust language breaking change)
- **Minor (1.2 → 1.3):** New exercises, improved explanations
- **Patch (1.2.1 → 1.2.2):** Bug fixes in examples, typos

**Artifact lineage:** Each version links to prior version, with changelog. Learners can see "what changed since I learned this."

**Meta-learning integration:** The Dojo learns from aggregate learner data ([educational data mining](https://learninganalytics.upenn.edu/ryanbaker/Chapter12BakerSiemensv3.pdf)) to improve artifacts.

**Sources:** [Educational data mining and learning analytics](https://www.researchgate.net/publication/316628053_Educational_data_mining_and_learning_analytics)

---

## 4. System Architecture

### 4.1 Four-Layer Design

```
┌─────────────────────────────────────────────────────────┐
│ LAYER 4: Feedback Loop (Performance → Artifact Update) │
│  - Failure pattern detection                           │
│  - Time-to-mastery tracking                            │
│  - Skill artifact versioning                           │
└─────────────────────────────────────────────────────────┘
                            ▲
                            │ Performance data
                            │
┌─────────────────────────────────────────────────────────┐
│ LAYER 3: Crystallization (Session Data → Skill)        │
│  - Cross-session pattern synthesis                     │
│  - Knowledge externalization                           │
│  - Competency ontology construction                    │
└─────────────────────────────────────────────────────────┘
                            ▲
                            │ Observations, episodes, summaries
                            │
┌─────────────────────────────────────────────────────────┐
│ LAYER 2: Memory (Session → Long-term Storage)          │
│  - Observation capture (file reads, errors, edits)     │
│  - Episode detection (work unit boundaries)            │
│  - Session summarization (intent, learned, next_steps) │
│  - Stance tracking (5D: phase/scope/locus/novelty/F)   │
└─────────────────────────────────────────────────────────┘
                            ▲
                            │ Hook events (SessionStart, PostToolUse, Stop)
                            │
┌─────────────────────────────────────────────────────────┐
│ LAYER 1: Practice (Human + AI → Active Learning)       │
│  - Structured sessions (tutorial, kata, challenge)     │
│  - Adaptive scaffolding (ZPD-aligned)                  │
│  - Immediate feedback (error correction)               │
│  - After-action review (reflection)                    │
└─────────────────────────────────────────────────────────┘
```

**Data flow:**
1. **Practice sessions** generate observations (tool uses, errors, successes)
2. **Memory layer** stores observations, detects episodes (work units), summarizes sessions
3. **Crystallization layer** synthesizes cross-session patterns into skill artifacts
4. **Feedback layer** tracks artifact usage, detects failure modes, triggers updates

### 4.2 Adaptive Scaffolding

The Dojo adjusts support based on **real-time competency estimation** (Bayesian Knowledge Tracing):

**Inputs:**
- **Performance history:** Success/failure on exercises per concept
- **Time-to-solution:** Decreasing time → automation (schema consolidation)
- **Error types:** Syntax errors (novice) vs logic errors (competent) vs design issues (proficient)
- **Stance signals:** High friction (failures > 0) → outside ZPD, reduce difficulty

**Scaffolding levels:**
| Level | Support Provided | Trigger Condition |
|-------|------------------|-------------------|
| **Heavy** | Step-by-step, worked examples, frequent hints | P(mastery) < 0.3, high error rate |
| **Moderate** | Guiding questions, partial solutions | 0.3 ≤ P(mastery) < 0.7 |
| **Light** | Verification only, challenge problems | P(mastery) ≥ 0.7 |
| **None** | Open-ended design, peer review | P(mastery) ≥ 0.9, teaching others |

**Adaptation triggers:**
- **Struggling (3+ failures on same concept):** Increase scaffolding, provide simpler sub-problem, offer worked example
- **Breezing (3+ rapid successes):** Decrease scaffolding, introduce variation, advance to next difficulty
- **Stuck (no progress for >10 min):** Agent intervenes: "What are you trying to do? Let's break it down."

**Implementation (nmem integration):**
- **Observation stream:** Each exercise generates observations (file_edit, command, error)
- **Real-time stance:** Compute phase × scope for current episode — high think+diverge → exploring, needs guidance
- **Knowledge tracing update:** P(mastery | success) via Bayes rule, stored in skill_progress table

**Sources:** [Adaptive learning systems](https://link.springer.com/article/10.1007/s44217-025-00908-6), [Zone of Proximal Development](https://www.simplypsychology.org/zone-of-proximal-development.html)

### 4.3 Session Types and Protocols

The Dojo supports multiple session modalities, each with distinct protocols:

#### Tutorial Session (Novice → Competent)
**Goal:** Learn new concept through guided exploration

**Protocol:**
1. **Setup (5 min):** Agent presents concept, real-world motivation, mental model
2. **Demonstration (10 min):** Agent writes code, narrates thought process (cognitive apprenticeship modeling)
3. **Guided practice (20 min):** Learner writes code, agent provides real-time feedback
4. **Independent practice (15 min):** Learner solves variation without agent intervention
5. **AAR (10 min):** Structured reflection (see [1.6](#16-simulation-training-and-after-action-reviews))

**Memory capture:**
- Marker: `tutorial:concept_name` with session summary
- Observations: file_read (reference docs), file_edit (code written), command (compile/test), errors (misconceptions)
- Session summary: intent, learned (key insights), next_steps

#### Kata Session (Competent → Proficient)
**Goal:** Automate schemas through spaced, varied practice

**Protocol:**
1. **Problem statement (2 min):** Agent presents kata, constraints (if any)
2. **Timed practice (20 min):** Learner solves independently, agent silent
3. **Review (10 min):** Agent compares learner's solution to expert patterns, highlights idioms
4. **Reflection (5 min):** "What would you do differently next time?"

**Memory capture:**
- Marker: `kata:kata_name:attempt_N`
- Performance metrics: time-to-solution, error count, pass/fail
- Solution diff: compare to prior attempts, track improvement

#### Challenge Session (Proficient → Expert)
**Goal:** Apply knowledge to novel, open-ended problems

**Protocol:**
1. **Problem definition (10 min):** Ambiguous requirements, learner must clarify
2. **Design phase (30 min):** Learner proposes architecture, agent critiques (Socratic method)
3. **Implementation (60 min):** Learner builds, agent observes passively
4. **Code review (20 min):** Agent identifies edge cases, performance issues, style violations
5. **Iteration (optional):** Learner refactors based on feedback

**Memory capture:**
- Marker: `challenge:problem_name`
- Design artifacts: architecture diagrams, decision rationale (captured in markdown)
- Code metrics: cyclomatic complexity, test coverage, performance benchmarks

#### Teaching Session (Expert verification)
**Goal:** Externalize knowledge by teaching others

**Protocol:**
1. **Content creation:** Learner writes tutorial/skill artifact for a concept
2. **Peer review:** Agent (or another learner) follows tutorial, reports gaps/errors
3. **Iteration:** Learner refines based on feedback
4. **Publication:** Artifact added to Dojo library

**Memory capture:**
- Artifact version 1.0.0 created
- Peer feedback logs
- Revision history

### 4.4 Competency Progression Tracking

The Dojo maintains a **skills matrix** ([competency progression model](https://www.iseazy.com/blog/competency-matrix/)) for each learner:

**Schema (SQLite table):**
```sql
CREATE TABLE learner_competency (
    learner_id TEXT NOT NULL,
    skill_id TEXT NOT NULL,  -- e.g., 'rust-lifetimes'
    concept_id TEXT NOT NULL,  -- e.g., 'lifetime-elision'
    level TEXT NOT NULL,  -- 'novice' | 'advanced_beginner' | 'competent' | 'proficient' | 'expert'
    p_mastery REAL NOT NULL,  -- Bayesian estimate [0,1]
    last_practiced INTEGER NOT NULL,  -- Unix timestamp
    next_review INTEGER,  -- Spaced repetition schedule
    total_attempts INTEGER DEFAULT 0,
    successful_attempts INTEGER DEFAULT 0,
    avg_time_seconds REAL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (learner_id, skill_id, concept_id)
);
```

**Update logic (after each exercise):**
```python
def update_competency(learner_id, skill_id, concept_id, success, time_seconds):
    # Bayesian Knowledge Tracing (simplified)
    prior_p = get_p_mastery(learner_id, skill_id, concept_id)

    if success:
        # P(mastery | success) via Bayes rule
        # Assume P(success | mastery) = 0.95, P(success | no_mastery) = 0.3
        likelihood = 0.95 * prior_p / (0.95 * prior_p + 0.3 * (1 - prior_p))
        new_p = min(likelihood, 0.99)  # Cap at 0.99 to allow forgetting
    else:
        # P(mastery | failure)
        likelihood = 0.05 * prior_p / (0.05 * prior_p + 0.7 * (1 - prior_p))
        new_p = likelihood

    # Update level based on thresholds
    if new_p >= 0.9 and avg_time < expert_threshold:
        level = 'expert'
    elif new_p >= 0.7:
        level = 'proficient'
    elif new_p >= 0.5:
        level = 'competent'
    elif new_p >= 0.3:
        level = 'advanced_beginner'
    else:
        level = 'novice'

    # Schedule next review (spaced repetition)
    next_review = compute_next_review(new_p, last_practiced)

    db.update(learner_id, skill_id, concept_id, level, new_p, next_review, time_seconds)
```

**Visualization:** Heatmap showing mastery across all concepts, highlighting gaps.

**Sources:** [Competency Matrix: What It Is and How to Use It](https://www.iseazy.com/blog/competency-matrix/), [Skills matrix: Track your team competency](https://www.scilife.io/blog/skills-matrix-competences)

### 4.5 Integration with nmem

The Dojo leverages nmem's existing infrastructure:

**Observation capture (already built):**
- `PostToolUse` hook → extract tool type, content, file_path, errors
- S2 classifiers → phase (think/act), scope (converge/diverge), locus, novelty
- S4 episode detection → work unit boundaries

**New Dojo-specific extensions:**

1. **`dojo_sessions` table:**
```sql
CREATE TABLE dojo_sessions (
    session_id TEXT PRIMARY KEY,
    learner_id TEXT NOT NULL,
    skill_id TEXT NOT NULL,
    session_type TEXT NOT NULL,  -- 'tutorial' | 'kata' | 'challenge' | 'teaching'
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    goal TEXT,  -- e.g., "Learn Rust lifetimes"
    outcome TEXT,  -- 'mastered' | 'progressed' | 'struggled' | 'abandoned'
    reflection TEXT,  -- AAR notes
    FOREIGN KEY (session_id) REFERENCES sessions(session_id)
);
```

2. **`dojo_exercises` table:**
```sql
CREATE TABLE dojo_exercises (
    exercise_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    skill_id TEXT NOT NULL,
    concept_id TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    completed_at INTEGER,
    success BOOLEAN,
    time_seconds INTEGER,
    error_count INTEGER,
    hints_used INTEGER,
    solution_diff TEXT,  -- Diff between learner's solution and reference
    FOREIGN KEY (session_id) REFERENCES dojo_sessions(session_id)
);
```

3. **`skill_artifacts` table:**
```sql
CREATE TABLE skill_artifacts (
    artifact_id TEXT PRIMARY KEY,
    skill_id TEXT UNIQUE NOT NULL,
    version TEXT NOT NULL,  -- SemVer
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    content TEXT NOT NULL,  -- TOML or JSON
    changelog TEXT,
    status TEXT NOT NULL,  -- 'draft' | 'published' | 'deprecated'
    author_id TEXT,  -- Learner who created (for teaching sessions)
    total_learners INTEGER DEFAULT 0,
    avg_time_to_mastery_hours REAL
);
```

**MCP tools (new):**
- `queue_dojo_session(skill_id, session_type, goal)` → Schedule session via S4 dispatcher
- `get_competency_matrix(learner_id, skill_id?)` → Retrieve current mastery levels
- `suggest_next_practice(learner_id)` → Spaced repetition scheduler returns due exercises
- `synthesize_skill_artifact(skill_id)` → Trigger crystallization layer

**CLI commands:**
```bash
nmem dojo start --skill rust-lifetimes --type tutorial  # Start new session
nmem dojo status --learner <id>  # Show competency matrix
nmem dojo review --skill rust-ownership  # What's due for review?
nmem dojo crystallize --skill rust-lifetimes  # Generate skill artifact from session history
```

---

## 5. Crystallization Algorithms

### 5.1 Pattern Synthesis from Session History

**Goal:** Transform N tutorial sessions on "Rust lifetimes" into a coherent skill artifact.

**Algorithm (pseudocode):**
```python
def crystallize_skill(skill_id: str) -> SkillArtifact:
    # 1. Gather all sessions for this skill
    sessions = db.query("""
        SELECT session_id, goal, learned, next_steps, reflection
        FROM dojo_sessions
        WHERE skill_id = ?
    """, skill_id)

    # 2. Extract concepts mentioned across sessions (FTS5 search)
    concepts = set()
    for session in sessions:
        concepts.update(extract_concepts(session['learned']))

    # 3. Build concept dependency graph
    graph = ConceptGraph()
    for concept in concepts:
        prerequisites = infer_prerequisites(concept, sessions)
        graph.add_node(concept, prerequisites)

    # 4. Cluster exercises by concept (via observations)
    exercises = db.query("""
        SELECT e.*, o.content, o.file_path
        FROM dojo_exercises e
        JOIN observations o ON e.session_id = o.session_id
        WHERE e.skill_id = ?
    """, skill_id)

    concept_to_exercises = cluster_by_concept(exercises, concepts)

    # 5. Identify common failure modes
    failures = [ex for ex in exercises if not ex['success']]
    failure_patterns = pattern_mine(failures)  # E.g., "80% fail when lifetimes in structs"

    # 6. Extract mental models from 'learned' fields
    mental_models = []
    for session in sessions:
        insights = parse_learned_field(session['learned'])
        mental_models.extend(insights)
    mental_models = deduplicate_and_rank(mental_models)

    # 7. Compute performance baselines
    avg_time_to_mastery = compute_time_to_mastery(exercises)
    difficulty_ratings = rate_exercises_by_failure_rate(exercises)

    # 8. Generate progression pathway (topological sort of dependency graph)
    ordered_concepts = graph.topological_sort()

    # 9. Create artifact structure
    artifact = SkillArtifact(
        skill_id=skill_id,
        version="1.0.0",
        concepts=ordered_concepts,
        exercises=concept_to_exercises,
        mental_models=mental_models,
        failure_patterns=failure_patterns,
        baselines={'avg_time_hours': avg_time_to_mastery},
        spaced_schedule=generate_spaced_schedule(ordered_concepts)
    )

    return artifact
```

**Key techniques:**
- **Concept extraction:** FTS5 search over `learned` fields for domain terms (e.g., "lifetime", "borrow", "elision")
- **Dependency inference:** If concept B appears in sessions only after concept A mastered → A prerequisite for B
- **Pattern mining:** [Sequential pattern mining](https://learninganalytics.upenn.edu/ryanbaker/Chapter12BakerSiemensv3.pdf) (educational data mining) to find common error sequences
- **Mental model dedup:** Semantic clustering (sentence embeddings + cosine similarity) to merge similar insights

### 5.2 LLM-Assisted Synthesis

For complex skills, use LLM to generate narrative structure:

**Prompt template:**
```
You are an expert educator synthesizing a skill artifact for [SKILL_NAME].

Input data:
- Concept dependency graph: [JSON]
- Top 10 mental models from learners: [LIST]
- Common failure patterns: [LIST]
- Exercise performance baselines: [JSON]

Task: Generate a skill artifact with:
1. Introduction explaining the skill's purpose and prerequisites
2. Progression pathway (Novice → Expert) with learning objectives per level
3. Recommended exercises per level (reference IDs from input data)
4. Mental models section with conceptual explanations
5. Common pitfalls and how to avoid them
6. Spaced practice schedule

Output format: TOML
```

**Human-in-the-loop:** LLM generates draft, learner (or original tutorial session author) reviews and refines.

### 5.3 Versioning and Continuous Improvement

**Trigger 1: Failure pattern emerges**
- If >50% of learners fail a new exercise not in current artifact → add to artifact, adjust difficulty rating
- If failure rate on existing exercise spikes → investigate (domain change? exercise bug?), update artifact

**Trigger 2: Domain evolution**
- Rust 2027 edition changes syntax → scan artifact for affected examples, queue update task

**Trigger 3: Performance drift**
- Average time-to-mastery increases >20% from baseline → exercises may be too hard, or prerequisites unclear
- Re-run dependency inference, check if new prerequisite emerged

**Versioning workflow:**
```python
def version_artifact(artifact_id: str, changes: List[Change]) -> str:
    current = load_artifact(artifact_id)

    # Classify change severity
    if any(c.is_breaking for c in changes):
        new_version = bump_major(current.version)
    elif any(c.is_feature for c in changes):
        new_version = bump_minor(current.version)
    else:
        new_version = bump_patch(current.version)

    # Apply changes
    updated = apply_changes(current, changes)
    updated.version = new_version
    updated.updated_at = now()
    updated.changelog = generate_changelog(current, updated)

    # Notify active learners
    learners = db.query("SELECT learner_id FROM learner_competency WHERE skill_id = ?", artifact_id)
    for learner in learners:
        notify(learner, f"Skill artifact {artifact_id} updated to {new_version}: {updated.changelog}")

    return save_artifact(updated)
```

---

## 6. Practical Design Patterns

### 6.1 Session Initiation Protocol

**User command:** `nmem dojo start --skill rust-lifetimes --type tutorial`

**System workflow:**
1. **Check prerequisites:** Query `learner_competency` → is `rust-ownership` mastered? If not, suggest: "You should complete 'rust-ownership' first."
2. **Load artifact:** Retrieve latest version of `rust-lifetimes` skill artifact
3. **Estimate ZPD:** Based on current P(mastery) for related concepts, select starting difficulty
4. **Generate session plan:**
   - Goal: "Understand lifetime syntax and elision rules"
   - Exercises: 3 from artifact (scaled to estimated level)
   - Time budget: 60 minutes
5. **Create session record:** Insert into `dojo_sessions`, mark `started_at`
6. **Launch agent:** Inject session plan into AI agent's context, begin tutorial protocol

**Context injection (to AI agent):**
```markdown
# Dojo Session: Rust Lifetimes (Tutorial)

**Learner:** <learner_id>
**Current level:** Advanced Beginner (P(mastery) = 0.4)
**Goal:** Understand lifetime syntax, annotate simple functions
**Exercises:** lifetime-basics-01, lifetime-basics-02, lifetime-basics-03
**Time budget:** 60 minutes

## Protocol:
1. Introduction (5 min): Explain what lifetimes are and why Rust needs them
2. Demonstration (10 min): Show lifetime annotations in a simple function
3. Guided practice (20 min): Learner annotates 2 functions, you provide real-time feedback
4. Independent practice (15 min): Learner solves lifetime-basics-03 solo
5. AAR (10 min): Reflect on what was learned, identify gaps

## Mental models to convey:
- Lifetimes are compile-time only (zero runtime cost)
- Lifetime elision rules reduce annotation burden
- Borrow checker prevents dangling references

## Common pitfalls:
- Confusing lifetimes with scope
- Over-annotating (elision handles most cases)

Begin the session.
```

### 6.2 Real-Time Feedback Mechanisms

**Observation → Feedback latency: <5 seconds**

**Trigger 1: Compilation error**
```python
# Hook: PostToolUse (Bash command = cargo build)
if 'error[E0106]: missing lifetime specifier' in tool_response:
    hint = skill_artifact.get_hint('E0106')  # Pre-authored in artifact
    agent.send_message(f"💡 Hint: {hint}")
    log_intervention('hint_provided', 'E0106')
```

**Trigger 2: Stuck (no progress >5 min)**
```python
# Monitored by dispatcher (S4)
if time_since_last_edit > 300 and not exercise_complete:
    agent.send_message("You've been on this for 5 minutes. What's blocking you? Let's break it down.")
    log_intervention('stuck_check')
```

**Trigger 3: Incorrect pattern (not an error, but non-idiomatic)**
```python
# Static analysis post-commit
if code_contains('unwrap()') and skill_id == 'rust-error-handling':
    agent.send_message("✋ You're using `unwrap()`. In production code, prefer `?` or `match`. Want to refactor?")
    log_intervention('style_nudge', 'unwrap_detected')
```

### 6.3 After-Action Review (AAR) Structure

**Timing:** Last 10 minutes of every session

**Agent-led prompts:**
1. **Reaction phase:**
   - "How do you feel about this session? What surprised you?"
   - (Affective state capture — frustration, boredom, engagement)

2. **Analysis phase:**
   - "What was the goal? Did we achieve it?"
   - "What worked well? Where did you struggle?"
   - "What's one key insight you'll remember?"

3. **Application phase:**
   - "How will you use this in your next project?"
   - "What should we focus on next session?"

**Captured in session record:**
```sql
UPDATE dojo_sessions
SET ended_at = ?,
    outcome = ?,  -- 'mastered' | 'progressed' | 'struggled'
    reflection = ?  -- AAR transcript
WHERE session_id = ?
```

**Post-session actions:**
- **Update competency matrix** based on exercise performance
- **Schedule next review** (spaced repetition)
- **Queue next session** if learner wants to continue
- **Synthesize marker:** `tutorial:rust-lifetimes:session-03 — Covered elision rules, struggled with struct lifetimes. Next: practice with method lifetimes.`

### 6.4 Spaced Practice Scheduler

**Algorithm:** Modified SM-2 (SuperMemo 2) with Bayesian updates

**Inputs:**
- Current P(mastery) for concept
- Days since last practice
- Performance on last exercise (success, time)

**Output:** Next review date

```python
def compute_next_review(concept_id, p_mastery, days_since_last, last_performance):
    # SM-2 easiness factor (EF)
    if last_performance == 'easy':
        EF = 2.5
    elif last_performance == 'medium':
        EF = 1.8
    else:  # hard
        EF = 1.3

    # Interval calculation
    if p_mastery < 0.5:
        interval_days = 1  # Daily practice until competent
    elif p_mastery < 0.7:
        interval_days = min(3 * EF, 7)  # Up to weekly
    elif p_mastery < 0.9:
        interval_days = min(14 * EF, 30)  # Up to monthly
    else:  # mastered
        interval_days = min(30 * EF, 90)  # Up to quarterly

    next_review = now() + timedelta(days=interval_days)
    return next_review
```

**Daily check (dispatcher job):**
```bash
# Cron: 9 AM daily
nmem dojo suggest-reviews --learner <id>
```

**Output:**
```
📚 Time to review!

Concept: Rust Lifetimes (last practiced 7 days ago)
Estimated time: 15 minutes
Exercise: lifetime-basics-02 (you got this in 12 min last time — can you beat it?)

Start now? [y/n]
```

### 6.5 Competency Badge System

**Visual progression markers** (like martial arts belts):

**Badge levels (per skill):**
- 🥚 **Novice** — P(mastery) ≥ 0.3, completed ≥3 exercises
- 🐣 **Advanced Beginner** — P(mastery) ≥ 0.5, completed ≥10 exercises
- 🐥 **Competent** — P(mastery) ≥ 0.7, completed ≥20 exercises, avg_time < 1.5× baseline
- 🦅 **Proficient** — P(mastery) ≥ 0.9, completed ≥30 exercises, avg_time < baseline
- 🏆 **Expert** — P(mastery) ≥ 0.95, created teaching artifact, peer-reviewed

**Display:**
```bash
nmem dojo status

Skills Matrix:
┌────────────────────┬───────┬───────────┬──────────┐
│ Skill              │ Badge │ Mastery   │ Due      │
├────────────────────┼───────┼───────────┼──────────┤
│ Rust Ownership     │ 🦅    │ 92%       │ 5 days   │
│ Rust Lifetimes     │ 🐥    │ 73%       │ today    │
│ Rust Traits        │ 🐣    │ 48%       │ 2 days   │
│ Async Rust         │ 🥚    │ 31%       │ tomorrow │
└────────────────────┴───────┴───────────┴──────────┘
```

---

## 7. Pitfalls and Mitigations

### 7.1 Over-Automation (Loss of Human Agency)

**Risk:** The Dojo becomes too prescriptive, learner becomes passive.

**Mitigation:**
- **Learner control:** Allow skipping exercises, choosing session type, requesting harder/easier problems
- **Metacognitive prompts:** "Why do you think you struggled here?" forces active reflection
- **Open-ended challenges:** Proficient+ levels have no "right answer," learner must defend design choices

**Research:** [ITS can reduce human interaction](https://pmc.ncbi.nlm.nih.gov/articles/PMC12078640/), which is critical for motivation. The Dojo must preserve learner autonomy.

### 7.2 Overfitting to Learner's Weaknesses

**Risk:** Spaced repetition only reviews past topics, never introduces new material.

**Mitigation:**
- **Exploration quota:** 20% of session time on new concepts (exploration), 80% on review (exploitation)
- **Prerequisite completion gates:** Can't advance to level N+1 until level N mastered, ensuring breadth

### 7.3 Skill Artifact Staleness

**Risk:** Artifact becomes outdated (domain evolves, failure patterns change).

**Mitigation:**
- **Version monitoring dashboard:** Track artifact age, failure rate trends
- **Community contributions:** Learners can submit PRs to artifact repo (like open-source docs)
- **Automated domain monitoring:** Scrape Rust release notes, flag potential breaking changes

### 7.4 False Mastery (Gaming the System)

**Risk:** Learner memorizes exercise solutions without understanding.

**Mitigation:**
- **Varied exercises:** Each review uses different problem (same concept, different context)
- **Transfer tests:** "You mastered lifetimes in functions — now apply to async code"
- **Code review challenges:** "Critique this code" — can't memorize critiques

### 7.5 Cognitive Overload (Too Much Tracking)

**Risk:** Learner overwhelmed by competency matrix, badges, schedules.

**Mitigation:**
- **Progressive disclosure:** Novices see only current exercise, Experts see full matrix
- **Opt-in analytics:** "Want to see your progress stats?" vs forcing dashboard
- **Silence mode:** "Just practice, no tracking" option for flow state

---

## 8. Implementation Roadmap

### Phase 1: Foundation (MVP)
**Goal:** Single-skill Dojo for Rust lifetimes

**Deliverables:**
1. Database schema (`dojo_sessions`, `dojo_exercises`, `learner_competency`)
2. Tutorial session protocol (5-phase: intro, demo, guided, independent, AAR)
3. Basic competency tracking (success/failure, time)
4. Manual skill artifact (TOML file, hand-authored)
5. MCP tool: `queue_dojo_session`, `get_competency_matrix`

**Validation:** 1 learner completes Rust lifetimes tutorial sequence (novice → competent), demonstrates mastery on challenge problem.

**Estimated effort:** 2-3 weeks

### Phase 2: Spaced Practice
**Goal:** Automated review scheduling

**Deliverables:**
1. Spaced repetition scheduler (SM-2 algorithm)
2. Dispatcher job: daily review suggestions
3. Exercise variation system (same concept, different problems)
4. Performance baseline tracking (avg_time_to_mastery)

**Validation:** Learner completes 1-month review cycle, retention verified via transfer test.

**Estimated effort:** 1-2 weeks

### Phase 3: Crystallization
**Goal:** Generate skill artifacts from session history

**Deliverables:**
1. Pattern synthesis algorithm (concept extraction, dependency inference)
2. LLM-assisted artifact generation
3. Versioning system (SemVer, changelog)
4. CLI: `nmem dojo crystallize --skill <id>`

**Validation:** Auto-generated artifact for "Rust Error Handling" matches quality of hand-authored artifact (peer review).

**Estimated effort:** 3-4 weeks

### Phase 4: Adaptive Scaffolding
**Goal:** Real-time difficulty adjustment

**Deliverables:**
1. Bayesian Knowledge Tracing implementation
2. ZPD detection (friction signals → outside ZPD)
3. Scaffolding level adjustment (heavy → moderate → light → none)
4. Real-time hint system (error code → pre-authored hint)

**Validation:** Learner with high error rate receives easier problems, learner breezing through receives harder problems.

**Estimated effort:** 2-3 weeks

### Phase 5: Multi-Skill Ecosystem
**Goal:** Dojo library of 10+ skills

**Deliverables:**
1. Skill dependency graph (Rust traits depend on Rust lifetimes)
2. Cross-skill transfer tests ("Apply ownership concepts to async code")
3. Badge system (visual progression markers)
4. Community contribution workflow (PR to artifact repo)

**Validation:** Learner progresses from "Rust Novice" to "Rust Proficient" across 5 skills in 3 months.

**Estimated effort:** 4-6 weeks

### Phase 6: Meta-Learning
**Goal:** The Dojo learns from aggregate data

**Deliverables:**
1. Educational data mining (sequential pattern mining for failure modes)
2. Failure pattern detection → artifact auto-update triggers
3. Performance drift monitoring (baselines vs actual)
4. A/B testing framework (test artifact variations, measure outcomes)

**Validation:** Artifact version 2.0 (auto-generated from failure patterns) outperforms version 1.0 (manual) by 15% in avg_time_to_mastery.

**Estimated effort:** 3-4 weeks

---

## 9. Research Gaps and Open Questions

### 9.1 How to Detect Expert-Level Competency?

**Challenge:** Experts exhibit unconscious competence — hard to measure via exercises alone.

**Possible approaches:**
- **Teaching requirement:** Must create artifact that successfully teaches 3 other learners
- **Code review quality:** Identify subtle bugs/design issues in real-world code
- **Novel problem creation:** Design new kata for the skill

**Research needed:** What observable behaviors distinguish proficient from expert?

### 9.2 Optimal Spaced Repetition Intervals for Skills (Not Facts)

**Challenge:** SRS research focuses on declarative knowledge (vocab, dates). Procedural skills may have different optimal intervals.

**Hypothesis:** Skills decay slower than facts but require "warm-up" — first rep after long break is slow, second rep is fast.

**Experiment:** Compare 1d/3d/7d vs 1d/7d/30d intervals for Rust lifetimes retention.

### 9.3 Can LLMs Auto-Generate Effective Exercises?

**Challenge:** GPT-4 can generate code examples, but are they pedagogically sound?

**Evaluation criteria:**
- **Concept isolation:** Does exercise test one concept cleanly?
- **Difficulty calibration:** Is it appropriate for target level?
- **Variation:** Is it sufficiently different from prior exercises?

**Approach:** Generate 100 exercises, have expert educator rate quality, train classifier on ratings.

### 9.4 How to Measure Transfer (Skill → Real-World)?

**Challenge:** Learner masters kata, but can they apply to production code?

**Possible metrics:**
- **PR quality:** Contribute to open-source Rust project, measure review feedback
- **Debugging speed:** Time to fix real-world bug vs synthetic bug
- **Design critique:** Evaluate real API design vs toy example

**Gold standard:** Longitudinal study — track Dojo learners' job performance vs control group.

---

## 10. Conclusion

The Dojo system synthesizes 50+ years of learning science into a practical framework for AI-assisted skill acquisition:

- **Deliberate practice** (Ericsson) → Structured, goal-directed sessions with feedback
- **Zone of Proximal Development** (Vygotsky) → Adaptive scaffolding keeps learner in sweet spot
- **Spaced repetition** (Ebbinghaus, modern SRS) → Optimal review timing for retention
- **Schema theory** (Sweller) → Progressive complexity, chunking, automation
- **Situated learning** (Lave & Wenger) → Authentic contexts, cognitive apprenticeship
- **Mastery learning** (Bloom) → Competency gates, formative feedback loops
- **Knowledge crystallization** (Nonaka & Takeuchi) → Tacit → explicit via externalization

The four-layer architecture (Practice → Memory → Crystallization → Feedback) creates a **self-improving learning environment**: every session refines the skill artifact, which improves future learners' outcomes.

**Unique contributions:**
1. **Stance-based progression tracking** — 5D classification (phase/scope/locus/novelty/friction) reveals cognitive state beyond pass/fail
2. **Cross-session synthesis** — Memory layer (nmem) enables pattern detection across months, not just single sessions
3. **Skill artifact as first-class artifact** — Teachable documents that evolve via community contribution and data-driven updates
4. **Agent-learner symbiosis** — AI's memory continuity compensates for human forgetting; human's metacognition guides AI's instruction

**Next steps:**
1. Implement Phase 1 MVP (single-skill Rust lifetimes Dojo)
2. Validate with 1 learner (dogfooding — use Dojo to learn advanced Rust concepts)
3. Open questions: Run experiments on spaced intervals, LLM exercise generation quality
4. Scale: Build 5-skill ecosystem (ownership, lifetimes, traits, async, unsafe)
5. Publish: Share findings + skill artifacts as open-source knowledge base

The Matrix's Dojo was a fiction where Neo learned kung fu in seconds. The real Dojo won't be instant — but it can be **measurably more effective** than unstructured learning, preserving hard-won knowledge across sessions, compressing it into artifacts, and feeding performance data back to improve the system. "I know Rust" becomes not a claim, but a **verified competency** with a progression trail, battle-tested exercises, and a skill artifact that helps the next learner reach mastery faster.

---

## Sources

### Deliberate Practice & Expertise
- [Deliberate Practice and Acquisition of Expert Performance](https://pubmed.ncbi.nlm.nih.gov/18778378/)
- [Is the Deliberate Practice View Defensible?](https://pmc.ncbi.nlm.nih.gov/articles/PMC7461852/)
- [Deliberate Practice Theory](https://mededmentor.org/theory-database/theory-index/deliberate-practice-theory/)

### Skill Acquisition Models
- [Dreyfus Model of Skill Acquisition - Wikipedia](https://en.wikipedia.org/wiki/Dreyfus_model_of_skill_acquisition)
- [The Dreyfus Model of Skill Acquisition - Mindtools](https://www.mindtools.com/atdbxer/the-dreyfus-model-of-skill-acquisition/)
- [Bloom's Taxonomy - Wikipedia](https://en.wikipedia.org/wiki/Bloom's_taxonomy)
- [Bloom's Taxonomy of Educational Objectives - UIC](https://teaching.uic.edu/cate-teaching-guides/syllabus-course-design/blooms-taxonomy-of-educational-objectives/)

### Spaced Repetition & Memory
- [Spacing Repetitions Over Long Timescales](https://pmc.ncbi.nlm.nih.gov/articles/PMC5476736/)
- [Spaced Repetition and Retrieval Practice](https://journals.zeuspress.org/index.php/IJASSR/article/view/425)
- [Unlock Your Memory Potential with SRS](https://traverse.link/spaced-repetition/srs-spaced-repetition-system)

### Cognitive Science
- [Zone of Proximal Development - Simply Psychology](https://www.simplypsychology.org/zone-of-proximal-development.html)
- [Vygotsky's ZPD and Scaffolding](https://educationaltechnology.net/vygotskys-zone-of-proximal-development-and-scaffolding/)
- [Cognitive Load Theory](https://www.structural-learning.com/post/cognitive-load-theory-a-teachers-guide)
- [Chunking as a pedagogy](https://paulgmoss.com/2023/02/13/chunking-as-a-pedagogy/)
- [Cognitive Load Theory and Expert Scaffolding](https://link.springer.com/article/10.1007/s10648-024-09848-3)

### Learning Theories
- [Situated Learning - Lave & Wenger](https://infed.org/dir/welcome/jean-lave-etienne-wenger-and-communities-of-practice/)
- [Situated Learning Theory](https://opentext.wsu.edu/theoreticalmodelsforteachingandresearch/chapter/situated-learning-theory/)
- [A Practical Review of Mastery Learning](https://pmc.ncbi.nlm.nih.gov/articles/PMC10159400/)
- [Competency-based learning and formative assessment](https://www.tandfonline.com/doi/full/10.1080/03075079.2023.2217203)

### Training Systems
- [Code Kata](http://codekata.com/)
- [Coding Dojo](https://codingdojo.org/kata/)
- [Why You Should Do Coding Dojos](https://dev.to/stealthmusic/why-you-should-do-coding-dojos-3n4j)
- [Belt Ranking System in Martial Arts](https://www.andrewballmma.com/belt-ranking-system)
- [Flight Simulator Fidelity and Training Transfer](https://commons.erau.edu/ijaaa/vol5/iss1/6/)

### Intelligent Tutoring Systems
- [A systematic review of AI-driven ITS in K-12](https://pmc.ncbi.nlm.nih.gov/articles/PMC12078640/)
- [Generative AI and Its Impact on ITS](https://arxiv.org/abs/2410.10650)
- [AI in intelligent tutoring systems toward sustainable education](https://slejournal.springeropen.com/articles/10.1186/s40561-023-00260-y)
- [AI in adaptive education](https://link.springer.com/article/10.1007/s44217-025-00908-6)

### Knowledge Management
- [SECI Model of Knowledge Dimensions](https://en.wikipedia.org/wiki/SECI_model_of_knowledge_dimensions)
- [Tacit vs Explicit Knowledge](https://bloomfire.com/blog/implicit-tacit-explicit-knowledge/)
- [Knowledge Crystallization Cycle](https://arxiv.org/html/2603.10808)

### Educational Technology
- [Learning Analytics and Educational Data Mining](https://learninganalytics.upenn.edu/ryanbaker/Chapter12BakerSiemensv3.pdf)
- [Educational data mining and learning analytics](https://www.researchgate.net/publication/316628053_Educational_data_mining_and_learning_analytics)
- [Knowledge Graphs in Education](https://pmc.ncbi.nlm.nih.gov/articles/PMC10847940/)
- [Skills Ontology Framework](https://gloat.com/blog/skills-ontology-framework/)

### Assessment & Measurement
- [Competency Matrix: What It Is and How to Use It](https://www.iseazy.com/blog/competency-matrix/)
- [Skills matrix: Track your team competency](https://www.scilife.io/blog/skills-matrix-competences)
- [Competency-Based Assessment](https://cloudassess.com/blog/competency-based-assessment/)

### Simulation & Debriefing
- [Debriefing Techniques in Medical Simulation](https://www.ncbi.nlm.nih.gov/books/NBK546660/)
- [Do Team and Individual Debriefs Enhance Performance?](https://www.researchgate.net/publication/236068923_Do_Team_and_Individual_Debriefs_Enhance_Performance_A_Meta-Analysis)
- [Simulation debriefing in nursing education](https://pmc.ncbi.nlm.nih.gov/articles/PMC9912432/)

### Meta-Learning
- [Meta-learning approaches for learning-to-learn](https://www.sciencedirect.com/science/article/abs/pii/S0925231222004684)
- [Advances in Meta-Learning](https://www.geeksforgeeks.org/advances-in-meta-learning-learning-to-learn/)
- [Sharing to learn and learning to share](https://arxiv.org/abs/2111.12146)
