# The Dojo System: AI-Assisted Skill Acquisition and Crystallization

*Research synthesis for designing a training construct that transforms learning sessions into battle-tested reusable skills*

## Executive Summary

A "Dojo" system for AI-assisted skill acquisition operates on multiple interdependent layers:

1. **Practice Layer**: Human learner + AI agent engage in structured sessions (Rust tutorials, debugging, system design)
2. **Memory Layer**: Cross-session memory captures progression via observations, episodes, session summaries, stance data (phase/scope/locus/novelty/friction)
3. **Crystallization Layer**: Learning solidifies into teachable skill artifacts—documents encoding what was learned
4. **Improvement Layer**: Skills improve through use as the memory system tracks performance, feeding back into the Dojo

This document synthesizes research from cognitive science, educational psychology, machine learning, and software craftsmanship to provide a design framework grounded in empirical evidence and practical patterns.

---

## Part I: Theoretical Foundations

### 1.1 Skill Acquisition Models

#### Dreyfus Model: Five Stages of Competence

[The Dreyfus Model](https://en.wikipedia.org/wiki/Dreyfus_model_of_skill_acquisition) proposes that learners pass through five distinct stages: novice, advanced beginner, competent, proficient, and expert. The fundamental transformation is from **rule-based thinking to intuitive, experience-based performance**.

**Stage Characteristics:**

| Stage | Decision Making | Context Sensitivity | Performance |
|-------|----------------|---------------------|-------------|
| **Novice** | Context-free rules, step-by-step | Low | Slow, requires conscious effort |
| **Advanced Beginner** | Experience-based maxims + rules | Recognizes situational patterns | Still deliberate |
| **Competent** | Goal-directed routines, prioritization | Organizes information hierarchically | Can feel overwhelmed by options |
| **Proficient** | Intuitive situation recognition | Holistic pattern matching | Fluid but still thinks through responses |
| **Expert** | Pure intuition, no reflective decision-making | Deep contextual understanding | Automatic, effortless |

**Dojo Implication**: Track progression through these stages using stance data. A learner in "novice" exhibits high `think+diverge` (seeking rules), while "expert" shows high `act+converge` with low friction. The shift from explicit rule-following to pattern recognition is observable in session transcripts and error rates.

**Key Principle**: *As students become skilled, they depend less on abstract principles and more on concrete experience* ([Dreyfus Model](https://pmc.ncbi.nlm.nih.gov/articles/PMC2887319/)). This maps directly to the transition from declarative knowledge (facts) to procedural knowledge (compiled skills).

---

#### Anderson's ACT-R: Knowledge Compilation

[ACT-R](https://act-r.psy.cmu.edu/about/) models skill acquisition as a three-stage process: **declarative → knowledge compilation → procedural**.

**Knowledge Compilation Process:**

1. **Declarative Stage**: High working memory load, frequent errors, verbal mediation. Facts about the skill must be rehearsed consciously.
2. **Compilation Stage**: Two subprocesses occur:
   - **Composition**: Collapse sequences of productions (rules) into single productions
   - **Proceduralization**: Embed factual knowledge directly into productions
3. **Procedural Stage**: Task-specific production rules fire automatically, bypassing declarative retrieval

**Mechanism**: [Production compilation](https://pubmed.ncbi.nlm.nih.gov/12916582/) combines and specializes task-independent procedures into task-specific procedures. By substituting declarative retrievals into rules, general production rules compile into task-specific rules, speeding up the process by reducing production firings and memory retrievals.

**Dojo Implication**: Early sessions should capture **explicit rule applications** (declarative). Later sessions should show **automatic pattern application** (procedural). The Dojo tracks this transition through:
- **Think+diverge → Act+converge** stance shifts
- **Novel → Routine** classification changes
- **Friction reduction** as compilation progresses

**Chunking and Working Memory**: [Chunking](https://en.wikipedia.org/wiki/Chunking_(psychology)) improves performance by decreasing cognitive load—allowing working memory to handle more complex tasks. Two mechanisms circumvent working memory limits: schema acquisition (meaningful units) and automation of procedural knowledge.

---

### 1.2 Deliberate Practice Framework

[Ericsson's deliberate practice](https://pmc.ncbi.nlm.nih.gov/articles/PMC6731745/) explains expert performance as the result of prolonged, effortful activities designed to optimize improvement.

**Core Requirements:**

1. **Designed to improve key aspects of current performance**: Not just repetition—practice targets specific weaknesses
2. **Challenging and effortful**: The hallmark is discomfort; you're stretching beyond current ability
3. **Requires immediate feedback**: Knowing what went wrong and why
4. **Involves problem-solving and evaluation**: Reflection on performance
5. **Allows repeated performance to refine behavior**: Iteration with adjustment

**Code Kata Pattern**: [Code katas](http://codekata.com/) apply deliberate practice to software development. The point is not the solution but the *practice itself*—like martial arts kata, where you repeat a form many times, making incremental improvements.

**Dojo Implication**: Sessions must be **effortful** (not routine), provide **immediate feedback** (error messages, test failures, agent explanations), and allow **repetition** with variation. The memory system tracks:
- **Friction episodes** = points where deliberate practice occurred (encountering resistance)
- **Repeated patterns** = same file/concept revisited across sessions
- **Performance deltas** = reduction in time/errors on similar tasks

**Warning**: [Research debates](https://pmc.ncbi.nlm.nih.gov/articles/PMC7461852/) exist around deliberate practice—it's not the only factor in expertise (innate ability, opportunity, motivation matter). The Dojo should not assume practice alone guarantees mastery.

---

### 1.3 Mastery Learning and Bloom's Taxonomy

[Mastery Learning](https://en.wikipedia.org/wiki/Mastery_learning) emphasizes that students must achieve high competence (e.g., 90% accuracy) in prerequisite knowledge before advancing. Bloom demonstrated that with appropriate instruction, 80-90% of students could match the top 20% under traditional methods.

**Bloom's Taxonomy** ([hierarchical framework](https://www.simplypsychology.org/blooms-taxonomy.html)) structures learning objectives by complexity:

1. **Remember**: Recall facts
2. **Understand**: Explain ideas
3. **Apply**: Use knowledge in new situations
4. **Analyze**: Break down into parts
5. **Evaluate**: Make judgments
6. **Create**: Produce new work

**Dojo Implication**: Skills should be **hierarchical** with prerequisite dependencies. A "skill tree" structure (see §3.4) ensures learners don't attempt advanced topics without foundational competence. The Dojo tracks:
- **Assessment alignment**: Session tasks map to Bloom levels
- **Mastery thresholds**: Before advancing, learners must demonstrate competence (e.g., can implement without errors, can explain why)
- **Prerequisite enforcement**: Skills unlock only when dependencies are mastered

---

### 1.4 Zone of Proximal Development and Scaffolding

Vygotsky's [Zone of Proximal Development (ZPD)](https://www.simplypsychology.org/zone-of-proximal-development.html) is the gap between what a learner can do alone and what they can do with guidance. [Scaffolding](https://link.springer.com/article/10.1007/3-540-47987-2_75) provides temporary support that fades as competence grows.

**Key Principles:**

- The ZPD is neither a property of the environment nor the student—it's a property of the **interaction between the two**
- [Intelligent Tutoring Systems](https://link.springer.com/chapter/10.1007/3-540-47987-2_75) aim to maintain students within their ZPD through adaptive scaffolding
- Scaffolding fades over time: early sessions provide heavy guidance, later sessions minimal hints

**Faded Scaffolding**: [Backward fading](https://cogscisci.wordpress.com/wp-content/uploads/2019/08/sweller-guidance-fading.pdf) starts with fully worked examples, then progressively omits solution steps (last step first, then second-to-last, etc.). Learners gradually take over problem-solving.

**Dojo Implication**: The AI agent acts as the **More Competent Other**. Early sessions provide:
- **Full explanations** (worked examples)
- **Step-by-step guidance**
- **Immediate error correction**

Later sessions fade to:
- **Hints instead of answers**
- **Questions that prompt reflection**
- **Delayed feedback to encourage struggle**

The transition is tracked via **friction rates** and **novelty classifications**. When friction drops and tasks become routine, scaffolding can fade.

---

### 1.5 Cognitive Load Theory and Worked Examples

[Cognitive Load Theory](https://ppig.org/files/2003-PPIG-15th-shaffer.pdf) distinguishes three types of load:

1. **Intrinsic Load**: Complexity inherent to the material
2. **Extraneous Load**: Poorly designed instruction that wastes cognitive resources
3. **Germane Load**: Effort devoted to schema construction (productive learning)

**Worked Example Effect**: [Worked examples](https://en.wikipedia.org/wiki/Worked-example_effect) reduce cognitive load during skill acquisition by showing step-by-step solutions. This is "one of the earliest and probably the best known cognitive load reducing technique."

**Faded Examples**: [Research shows](https://www.tandfonline.com/doi/full/10.1080/01443410.2023.2273762) that worked examples should be faded over time—replaced with problems for practice. Backward fading (progressively omitting steps) optimizes learning efficiency.

**Dojo Implication**: Early skill acquisition uses **worked examples** (agent demonstrates, learner observes). Middle stages use **faded examples** (partial solutions). Advanced stages use **problem sets** (learner solves independently). The Dojo tracks:
- **Example-to-problem ratio** per session
- **Error rates** when scaffolding is reduced
- **Time to solution** as compilation progresses

---

### 1.6 Spaced Repetition and the Forgetting Curve

[Ebbinghaus's Forgetting Curve](https://en.wikipedia.org/wiki/Forgetting_curve) shows that without reinforcement, 50% of newly learned information is lost within an hour, 70% within 24 hours, 90% within a month.

**Spaced Repetition Systems (SRS)**: [SRS](https://en.wikipedia.org/wiki/Spaced_repetition) use statistical algorithms to schedule reviews at optimal intervals—just before the learner would forget. The **spacing effect** shows that distributed practice beats massed practice by 74%.

**Key Mechanisms:**

- **Active Recall**: Retrieval practice strengthens memory
- **Testing Effect**: Testing creates deeper encoding than re-reading
- **Optimal Timing**: Review when memory is fading but still retrievable

**Dojo Implication**: Skills should be **revisited at spaced intervals**. The Dojo schedules:
- **Immediate practice** (same session)
- **Short-term review** (next session, 1-3 days later)
- **Medium-term review** (1-2 weeks later)
- **Long-term maintenance** (monthly)

The memory system tracks **last practice date** and **performance decay** to trigger reviews. If a previously mastered skill shows errors after a gap, it's flagged for review.

---

### 1.7 Retrieval Practice and the Testing Effect

[The testing effect](https://en.wikipedia.org/wiki/Testing_effect) demonstrates that **retrieval practice produces more learning than restudying**. Actively recalling information strengthens memory traces.

**Mechanism**: [Effortful retrieval](http://psychnet.wustl.edu/memory/wp-content/uploads/2018/04/Roediger-Karpicke-2006_PPS.pdf) creates "desirable difficulties" that enhance long-term retention. Regular, low-stakes retrieval transforms classrooms from places where information is delivered to **places where knowledge is actively constructed**.

**Connection to Knowledge Construction**: Retrieval helps "create coherent and integrated mental representations of complex concepts, the kind of deep learning necessary to solve new problems and draw new inferences" ([Active Retrieval](https://learninglab.psych.purdue.edu/downloads/2012/2012_Karpicke_CDPS.pdf)).

**Dojo Implication**: Sessions should include **active recall tasks**:
- "Explain how X works without looking at notes"
- "Implement Y from memory"
- "Debug this code—what's wrong and why?"

The Dojo tracks **retrieval success rates** and schedules harder retrieval tasks as skills solidify.

---

### 1.8 Error-Driven Learning and Productive Failure

[Learning from errors](https://pmc.ncbi.nlm.nih.gov/articles/PMC11803059/) is essential for skill development. **Productive failure** occurs when "failure precedes later success in learning"—struggling with a problem before receiving instruction leads to deeper understanding.

**Key Findings:**

- [Kapur's research](https://bpspsychub.onlinelibrary.wiley.com/doi/10.1111/bjep.12716) shows that students in productive failure conditions score higher on posttests than those who receive instruction first
- **Impasses and errors are strongly associated with learning**—they trigger explanation and reflection processes
- **Negative knowledge** (knowing what *not* to do) complements positive knowledge

**Mechanism**: Errors initiate processes where deficient concepts are contrasted with correct concepts to establish accurate mental models.

**Dojo Implication**: The system should **expect and leverage errors**. Friction episodes (failures > 0) are **learning opportunities**, not failures of the system. The Dojo:
- **Captures error patterns** (same mistake repeated?)
- **Provides timely, specific feedback** after errors
- **Encourages reflection**: "What went wrong? Why?"
- **Tracks error recovery time** (improving = learning)

---

### 1.9 Meta-Learning: Learning to Learn

[Meta-learning](https://www.frontiersin.org/journals/education/articles/10.3389/feduc.2025.1697554/full) empowers systems to **learn how to learn**—acquiring knowledge from multiple tasks enables faster adaptation to new tasks.

**Cognitive Science Roots**: Meta-learning has deep roots in psychology and neuroscience, where organisms adapt learning strategies based on past experiences. A key feature of human intelligence is the ability to **transfer knowledge** learned from multiple tasks to similar but different ones.

**Recent Paradigm Shift**: The [Cognitive Mirror](https://www.frontiersin.org/journals/education/articles/10.3389/feduc.2025.1697554/full) paradigm reconceptualizes AI as a **teachable novice** that reflects the quality of a learner's explanation. This shifts from "knowledge transfer" to "knowledge construction."

**Dojo Implication**: The system should track **learning strategies** that work:
- "Learner prefers worked examples before practice"
- "Learner benefits from visual diagrams"
- "Learner needs 3 repetitions before mastery"

These meta-patterns become part of the skill artifact. When learning a new domain, the Dojo applies successful strategies from previous domains.

---

## Part II: Practical Pedagogical Patterns

### 2.1 Cognitive Apprenticeship

[Cognitive Apprenticeship](https://www.aft.org/ae/winter1991/collins_brown_holum) brings traditional apprenticeship into knowledge work. Four key methods:

1. **Modeling**: Expert demonstrates while making thinking visible
2. **Coaching**: Expert observes learner, provides hints/feedback
3. **Scaffolding**: Support fades as learner becomes proficient
4. **Fading**: Teacher becomes monitor, offering occasional hints

**Dojo Implication**: The AI agent is the **expert model**. Sessions follow the apprenticeship pattern:
- **Demonstration phase**: Agent shows how to solve problems, narrating thought process
- **Guided practice**: Learner attempts with agent coaching
- **Independent practice**: Learner works alone, agent monitors
- **Peer teaching**: Learner explains to agent (Feynman technique)

---

### 2.2 Code Dojo and Kata

[Coding Dojos](https://codingdojo.org/kata/) are group training sessions where programmers practice and improve through collaborative exercises. [Code katas](http://codekata.com/) are exercises in programming that help hone skills through practice and repetition.

**Key Principles:**

- **The point is practice, not the solution**: You may solve the same kata 100 times, each time learning something new
- **Focus on the process**: Reflect on *how* you solved it, not just *that* you solved it
- **Incremental improvement**: Small improvements each time
- **Deliberate discomfort**: Stretching beyond current ability

**Dojo Implication**: The system should provide **repeatable exercises**:
- "Implement a binary search tree in Rust" (kata)
- "Refactor this code to use iterators" (kata)
- "Debug this program" (kata)

Each repetition is tracked. The Dojo measures:
- **Time to solution** (should decrease)
- **Code quality** (fewer bugs, better patterns)
- **Approach variation** (trying different strategies)

---

### 2.3 Apprenticeship Patterns (Software Craftsmanship)

[Apprenticeship Patterns](https://www.oreilly.com/library/view/apprenticeship-patterns/9780596806842/) catalogs behavior patterns for software developers progressing from apprentice → journeyman → master.

**Key Patterns:**

- **Be the Worst**: Join a team where you're the least experienced—accelerates learning
- **Breakable Toys**: Build projects for yourself to learn without production pressure
- **Expose Your Ignorance**: Ask questions, admit what you don't know
- **The White Belt**: Approach new topics with beginner's mind
- **Nurture Your Passion**: Stay motivated through intrinsic interest

**Progression Model**: Apprentice (learning from masters) → Journeyman (spreading knowledge across teams) → Master (enhancing others' skills). **Mastery is a superset of apprenticeship**—you never stop learning.

**Dojo Implication**: The skill tree should reflect **progression stages**:
- **Apprentice skills**: Following patterns, guided practice
- **Journeyman skills**: Independent problem-solving, teaching others
- **Master skills**: Creating new patterns, mentoring

The Dojo tracks **teaching moments** (when learner explains to agent) as signs of journeyman progression.

---

### 2.4 Flight Simulator Training: Fidelity and Transfer

[Flight simulator research](https://commons.erau.edu/ijaaa/vol5/iss1/6/) distinguishes between **training effectiveness** and **prediction validity**:

- **Prediction Validity**: Simulator performance predicts real-world performance
- **Training Effectiveness**: Simulator accelerates learning

**Key Finding**: Devices effective for learning may not be valid predictors, and vice versa. High-fidelity simulators aren't always optimal for learning—sometimes low-fidelity focused practice is better.

**Fidelity Types:**

1. **Physical Fidelity**: Visual/tactile realism
2. **Functional Fidelity**: Behavioral realism
3. **Cognitive Fidelity**: Mental processes match real task

**Transfer Types:**

- **Self-Transfer**: Repeated practice of same event
- **Near Transfer**: Practice of very similar events
- **Far Transfer**: Practice of dissimilar events

**Dojo Implication**: Simulated exercises (coding problems) need **functional fidelity** (real constraints) but not necessarily **physical fidelity** (production environment). The Dojo should:
- **Start simple** (low fidelity, focused practice)
- **Increase complexity** (approach production realism)
- **Measure transfer**: Does training improve real-world performance?

---

### 2.5 Martial Arts Belt System: Visible Progression

[Martial arts belt systems](https://karateintokyo.com/karate_basics/karate-belt-ranking-system-guide/) provide **visual markers of progression**. Each belt color represents a specific stage of mastery.

**Assessment Components:**

1. **Kata Performance**: Pre-arranged patterns executed with precision, power, understanding
2. **Kumite (Sparring)**: Applying techniques against resistance
3. **Knowledge**: Oral/written exam on principles, history, philosophy
4. **Character**: Leadership, teaching ability (for advanced belts)

**Evaluation Process**: Students receive personalized feedback outlining areas for improvement. Evaluators look not just for accuracy but for **growth** at the current level.

**Dojo Implication**: Skills should have **visible progression markers**:
- **Beginner** (white belt): Can follow tutorials
- **Intermediate** (colored belts): Can solve problems independently
- **Advanced** (brown belt): Can debug complex issues
- **Expert** (black belt): Can design systems, teach others

The Dojo tracks progression through **competency assessments** tied to each level.

---

## Part III: System Design Patterns

### 3.1 Intelligent Tutoring Systems (ITS)

[Intelligent Tutoring Systems](https://link.springer.com/chapter/10.1007/3-540-47987-2_75) adapt content and activities to be both effective and efficient, maintaining students in their Zone of Proximal Development.

**ITS Components:**

1. **Domain Model**: What is being taught (skill structure)
2. **Student Model**: What the learner knows (knowledge tracing)
3. **Pedagogical Model**: How to teach (adaptive strategies)
4. **Interface**: How learner and system interact

**Adaptive Scaffolding**: Modern ITS use machine learning to analyze learner responses continuously, identify learning gaps, and adjust scaffolding in real-time.

**Dojo Implication**: The system needs:
- **Domain Model**: Skill tree with prerequisites
- **Learner Model**: Knowledge tracing based on observations, episodes, session summaries
- **Pedagogy**: Rules for when to provide examples vs. problems, when to fade scaffolding
- **Interface**: Sessions with the AI agent

---

### 3.2 Knowledge Tracing

[Bayesian Knowledge Tracing (BKT)](https://en.wikipedia.org/wiki/Bayesian_Knowledge_Tracing) models learner mastery as a hidden Markov process. It tracks binary learning states over time using parameters:

- **P(L₀)**: Initial probability of knowing skill
- **P(T)**: Probability of learning (transition from unknown to known)
- **P(S)**: Probability of slip (knows but makes error)
- **P(G)**: Probability of guess (doesn't know but gets correct)

**Modern Extensions**: [Deep Knowledge Tracing](https://stanford.edu/~cpiech/bio/papers/deepKnowledgeTracing.pdf) uses LSTMs to achieve AUC 0.85 vs. BKT's 0.68, handling more complex skill dependencies.

**Dojo Implication**: The memory system already captures observations with correctness signals (friction, errors). This data can feed a knowledge tracing model:
- **Input**: Sequence of attempts (observations) per skill
- **Output**: P(mastery) for each skill
- **Trigger**: When P(mastery) > 0.9, skill is "mastered"

---

### 3.3 Learning Analytics and Educational Data Mining

[Learning Analytics (LA) and Educational Data Mining (EDM)](https://files.eric.ed.gov/fulltext/ED611199.pdf) extract useful information from large educational datasets.

**Applications:**

- **Competency Tracking**: Monitor skill development during learning, not just final grades
- **Predictive Modeling**: Identify at-risk learners early
- **Adaptive Systems**: Adjust content based on performance data
- **Dashboards**: Visualize progress for learners and teachers

**SCALA Example**: An integrated analytics system that uses LA techniques to visualize enriched indicators to teachers and learners.

**Dojo Implication**: The nmem database is already an educational data mining goldmine:
- **Observations**: Tool use, errors, time spent
- **Episodes**: Work units with stance character
- **Session Summaries**: Intent, learned, completed, next_steps
- **Stance Data**: 5-dimensional classification

The Dojo should provide **dashboards** showing:
- **Skills mastered** vs. in progress
- **Learning velocity** (skills/week)
- **Friction hotspots** (skills with high error rates)
- **Progression trajectory** (Dreyfus stage over time)

---

### 3.4 Skill Trees and Prerequisite Graphs

[Skill trees](https://arxiv.org/html/2504.16966v1) are graph-based methods where content is split into skills (nodes) whose dependencies (edges) form a **directed acyclic graph (DAG)**.

**Structure:**

- **Skills**: Nodes representing something a learner should be able to do
- **Dependencies**: Edges showing prerequisites
- **Learning Paths**: Sequences through the graph

**Example**:
```
"Understand fractions" → "Write equivalent fractions" → "Add fractions"
```

**Implementation**: [SkillTree platform](https://skilltreeplatform.dev/overview/) enforces prerequisites—no points awarded for Skill B until Skill A is fully accomplished.

**Dojo Implication**: Skills should be **explicitly structured**:
- Define prerequisites (can't learn async Rust without understanding ownership)
- Lock advanced skills until prerequisites are mastered
- Generate personalized learning paths based on current mastery

The skill tree lives in the skill artifact. The Dojo tracks which nodes are unlocked for each learner.

---

### 3.5 Knowledge Crystallization and Externalization

[The Knowledge Crystallization Cycle (KCC)](https://arxiv.org/html/2603.10808) transforms fragmented, contextual knowledge from conversational interactions into **structured, reusable, transferable knowledge assets**.

**SECI Model** ([Nonaka & Takeuchi](https://ascnhighered.org/ASCN/change_theories/collection/seci.html)) describes knowledge transformation:

1. **Socialization**: Tacit to tacit (learning by doing together)
2. **Externalization**: Tacit to explicit (articulating knowledge)
3. **Combination**: Explicit to explicit (integrating knowledge sources)
4. **Internalization**: Explicit to tacit (practicing until automatic)

**Externalization Process**: When tacit knowledge is made explicit, it can be shared by others and becomes the basis of new knowledge—e.g., best practice guides, playbooks.

**Dojo Implication**: The system must **externalize** learned knowledge:
- **Sessions** (tacit practice) → **Session summaries** (explicit records)
- **Session summaries** → **Skill artifacts** (teachable documents)
- **Skill artifacts** → **Internalized skills** (practiced until routine)

The crystallization process is **documented in the skill artifact**: what was learned, how it was learned, what exercises worked, what pitfalls to avoid.

---

### 3.6 Living Documentation and Docs-as-Code

[Living Documentation](https://www.amazon.com/Living-Documentation-Cyrille-Martraire/dp/0134689321) changes at the same pace as software design/development. It solves the outdated documentation problem by **generating documentation directly from source code, tests, and executable specifications**.

**Docs-as-Code**: [Documentation as Code](https://konghq.com/blog/learning-center/what-is-docs-as-code) uses the same tools for writing and documenting code. CI/CD pipelines automate documentation deployment, ensuring it stays current.

**Key Principle**: Documentation is a **byproduct of work**, not an afterthought. When docs are integrated into the workflow, keeping them current requires no additional effort.

**Dojo Implication**: Skill artifacts should be **living documents**:
- Updated automatically when skills are practiced (new examples added)
- Linked to session transcripts (evidence of mastery)
- Versioned (track how the skill artifact evolves)
- Generated from structured data (not handwritten)

The Dojo generates skill artifacts by querying the memory system:
- "What sessions touched this skill?"
- "What worked examples were used?"
- "What errors did learners make?"
- "What explanations did the agent provide?"

---

### 3.7 Feedback Loops and Formative Assessment

[Feedback loops](https://pmc.ncbi.nlm.nih.gov/articles/PMC12086178/) connect real-time learning with formative assessment and future learning, creating an **iterative process** that promotes mutual accountability.

**Adaptive Feedback Mechanisms**: Real-time feedback improves engagement by 35.1%. Weekly formative assessments show 87.5% adjustment rate with 30.2% engagement improvement.

**Closed-Loop Ecosystem**: Integrates students, teachers, and AI into a feedback ecosystem driven by student behavior, model response, and adaptive instructional strategies.

**Dojo Implication**: The system must provide **tight feedback loops**:
- **Immediate**: Error messages, test failures during session
- **Session-level**: Summary at end of session (what you learned, what's next)
- **Skill-level**: Progress on skill tree (you're 70% toward mastering X)
- **Meta-level**: Learning velocity, Dreyfus stage progression

Feedback should be **actionable**: not just "you made an error" but "here's what to try next."

---

### 3.8 Active Learning and Query Strategies

[Active Learning](https://en.wikipedia.org/wiki/Active_learning_(machine_learning)) allows a learning algorithm to **interactively query** for the most informative data points to label.

**Query Strategies:**

- **Uncertainty Sampling**: Select examples where model is least confident
- **Diversity Sampling**: Select examples covering different regions of feature space
- **Query by Committee**: Multiple models vote; select examples where they disagree

**Relationship to Curriculum Learning**: In curriculum learning, human experts impose structure on examples. In active learning, the system chooses examples.

**Dojo Implication**: The system should **select exercises** strategically:
- **Uncertainty-based**: Practice skills where learner performance is inconsistent (P(mastery) ≈ 0.5)
- **Diversity-based**: Ensure coverage of skill tree (don't over-practice one area)
- **Committee-based**: If multiple strategies disagree on mastery, test more

---

## Part IV: Measurement and Assessment

### 4.1 Competency-Based Assessment

[Competency-based learning](https://en.wikipedia.org/wiki/Mastery_learning) assesses learning based on predetermined competencies, emphasizing **what learners can do** rather than time spent.

**Principles:**

- **Mastery before progression**: Can't advance until competence demonstrated
- **Clear standards**: Rubrics define what "mastery" means
- **Multiple pathways**: Different learners may reach competence differently
- **Formative feedback**: Ongoing assessment informs learning

**Dojo Implication**: Each skill has **mastery criteria**:
- "Can implement X without errors" (behavioral)
- "Can explain Y concept" (conceptual)
- "Can debug Z issue in < 10 minutes" (performance)

Assessment data comes from observations (did they do it?) and session summaries (did they understand it?).

---

### 4.2 Performance Tracking and Skill Retention

[The Forgetting Curve](https://en.wikipedia.org/wiki/Forgetting_curve) predicts performance decay without reinforcement. **Skill retention** depends on:

- **Material difficulty**: Complex skills decay faster
- **Meaningfulness**: Contextual, relevant skills retained longer
- **Practice spacing**: Distributed practice beats massed practice

**Retention Strategies:**

- **Spaced repetition**: Review at increasing intervals
- **Active recall**: Test yourself regularly
- **Contextual embedding**: Connect to meaningful projects

**Dojo Implication**: The system tracks **last practice date** for each skill. When time since practice exceeds a threshold (skill-dependent), trigger a review session:
- **Simple skills**: Review every 1-2 months
- **Complex skills**: Review every 2-4 weeks
- **Foundational skills**: Review quarterly

The Dojo measures **performance decay** by comparing current performance to past performance on the same task.

---

### 4.3 Stance Analysis: Cognitive Trajectory Tracking

The nmem system already tracks 5-dimensional stance data (phase, scope, locus, novelty, friction). This maps cleanly to skill acquisition stages.

**Stance Fingerprints by Dreyfus Stage:**

| Stage | Phase | Scope | Locus | Novelty | Friction |
|-------|-------|-------|-------|---------|----------|
| **Novice** | Think+Diverge | High | External | Novel | High |
| **Advanced Beginner** | Think+Converge | Medium | Mixed | Mixed | Medium |
| **Competent** | Act+Converge | Medium | Internal | Routine | Medium |
| **Proficient** | Act+Converge | Low | Internal | Routine | Low |
| **Expert** | Act (intuitive) | Converge | Internal | Routine | Very Low |

**Dojo Implication**: Stance trajectory shows **learning progress**. A learner on Rust:
- **Session 1-5**: Think+Diverge (reading docs, trying examples) → Novice
- **Session 6-15**: Think+Converge (solving problems with guidance) → Advanced Beginner
- **Session 16-30**: Act+Converge (implementing independently) → Competent
- **Session 31+**: Act+Converge, low friction → Proficient

The Dojo queries stance data to **estimate Dreyfus stage** and adjusts pedagogy accordingly.

---

### 4.4 Episode-Based Skill Assessment

The nmem system detects **episodes** (work units) and annotates them with:
- **Intent**: What was the goal?
- **Phase signature**: Think/act distribution
- **Hot files**: What was touched?
- **Friction**: Did failures occur?
- **Narrative**: What happened?

**Dojo Implication**: Episodes are **natural assessment units**. A skill is "mastered" when:
- **Episodes involving that skill show low friction** (successful completion)
- **Learner can complete task independently** (low think+diverge)
- **Performance is consistent** (multiple successful episodes)

The Dojo tags episodes with skills practiced. Skill mastery = N successful episodes (e.g., N=3).

---

## Part V: Dojo Architecture Design

### 5.1 Four-Layer Architecture

```
┌─────────────────────────────────────────────────────────┐
│  Layer 4: Improvement & Feedback                        │
│  - Skill performance tracking                           │
│  - Decay detection & review scheduling                  │
│  - Skill artifact updates                               │
└─────────────────────────────────────────────────────────┘
                          ↑
                          │ Performance data
                          ↓
┌─────────────────────────────────────────────────────────┐
│  Layer 3: Crystallization                               │
│  - Externalize tacit knowledge → skill artifacts        │
│  - Generate teachable documents from sessions           │
│  - Version & evolve skill definitions                   │
└─────────────────────────────────────────────────────────┘
                          ↑
                          │ Session summaries, episodes
                          ↓
┌─────────────────────────────────────────────────────────┐
│  Layer 2: Memory & Pattern Detection                    │
│  - Observations, episodes, session summaries            │
│  - Stance tracking (5 dimensions)                       │
│  - Cross-session pattern detection                      │
│  - Knowledge tracing (skill mastery estimation)         │
└─────────────────────────────────────────────────────────┘
                          ↑
                          │ Tool calls, errors, transcripts
                          ↓
┌─────────────────────────────────────────────────────────┐
│  Layer 1: Practice & Interaction                        │
│  - Human learner + AI agent sessions                    │
│  - Structured exercises (katas, worked examples)        │
│  - Real-time feedback (errors, explanations)            │
└─────────────────────────────────────────────────────────┘
```

**Data Flow:**

1. **Practice generates observations** (file reads, edits, errors)
2. **Observations aggregate into episodes** (work units with intent)
3. **Episodes compress into session summaries** (learned, completed, next steps)
4. **Summaries feed skill artifacts** (crystallized knowledge)
5. **Artifacts guide future practice** (review schedules, exercise selection)

---

### 5.2 Skill Artifact Structure

A skill artifact is a **living document** that captures everything about learning a skill. Proposed schema:

```markdown
# Skill: [Name]

## Metadata
- **ID**: unique identifier
- **Prerequisites**: [list of skill IDs]
- **Dreyfus Stage**: Competent (estimated from learner performance)
- **Mastery Criteria**: [behavioral tests for mastery]
- **Last Practiced**: 2026-03-01
- **Next Review**: 2026-04-01
- **Total Sessions**: 8
- **Avg Friction**: 0.15 (low)

## Learning Progression

### Novice Stage (Sessions 1-3)
- **Worked Examples**: [links to session transcripts]
- **Key Concepts**: ownership, borrowing, lifetimes
- **Common Errors**: "cannot borrow as mutable"
- **Agent Explanations**: [links to explanations given]

### Advanced Beginner (Sessions 4-6)
- **Exercises Completed**: [list]
- **Friction Points**: lifetime annotations, closure captures
- **Breakthroughs**: "understood the borrow checker is a proof system"

### Competent (Sessions 7-8)
- **Independent Projects**: [links to episodes]
- **Performance**: avg 15 min to implement standard pattern
- **Remaining Gaps**: async lifetimes

## Recommended Exercises
- **Kata 1**: Implement a doubly-linked list (tests ownership understanding)
- **Kata 2**: Build a simple parser (tests lifetime annotations)
- **Kata 3**: Refactor callback code to use iterators

## Pitfalls & Gotchas
- Forgetting `mut` in function signatures
- Confusing `&str` and `String`
- Fighting the borrow checker instead of restructuring code

## Resources
- [Official Rust Book chapters 4, 10, 15]
- [Session transcripts tagged with this skill]
- [Related skills: Rust Error Handling, Rust Traits]

## Next Steps
- Practice async Rust (prerequisite now met)
- Build a small HTTP server (real-world project)
```

**Generation Process:**

1. **Query nmem**: "Find all sessions where `tag:tutorial:rust-ownership` appears"
2. **Extract patterns**:
   - Common errors (via observations with `obs_type=command` and failures)
   - Worked examples (via file reads of tutorial code)
   - Explanations (via session transcripts with agent responses)
3. **Synthesize document**: Use LLM to generate narrative from structured data
4. **Version**: Track changes over time (skill artifact is itself version-controlled)

---

### 5.3 Knowledge Tracing Integration

**Current nmem data** → **Knowledge tracing input**:

| nmem Data | KT Usage |
|-----------|----------|
| `observations.friction` | Slip probability (knew but made error) |
| `sessions.summary.learned` | Evidence of learning transition (P(T)) |
| `episodes.friction_label` | Success/failure signal per work unit |
| `stance: act+converge + routine + low friction` | High P(mastery) |
| `stance: think+diverge + novel + high friction` | Low P(mastery) |

**Simple BKT Implementation:**

```python
# Pseudocode for Bayesian Knowledge Tracing per skill
def update_mastery(skill_id, observation):
    prior = get_mastery_estimate(skill_id)

    if observation.success:
        # Correct answer: increase mastery estimate
        posterior = bayesian_update(prior, P_guess=0.1, P_slip=0.1, correct=True)
    else:
        # Error: decrease mastery estimate
        posterior = bayesian_update(prior, P_guess=0.1, P_slip=0.1, correct=False)

    set_mastery_estimate(skill_id, posterior)

    if posterior > 0.9:
        mark_skill_mastered(skill_id)
        unlock_dependent_skills(skill_id)
```

**Triggers:**

- After each episode involving a skill, update mastery estimate
- When P(mastery) > 0.9 for N consecutive observations, mark mastered
- When mastered, unlock dependent skills in skill tree

---

### 5.4 Adaptive Scaffolding Rules

**Pedagogy Model** (rules for adjusting support):

| Condition | Action |
|-----------|--------|
| P(mastery) < 0.3 | Provide worked examples, full explanations |
| 0.3 ≤ P(mastery) < 0.6 | Faded scaffolding: hints, partial solutions |
| 0.6 ≤ P(mastery) < 0.9 | Independent practice, delayed feedback |
| P(mastery) ≥ 0.9 | Challenge problems, teach-back (explain to agent) |
| Friction spike after gap | Review session with worked examples |
| Repeated errors on same concept | Switch strategy (visual explanation? Different exercise?) |

**Fading Schedule:**

```
Session 1: Agent demonstrates (100% scaffolding)
Session 2: Agent demonstrates + learner tries with guidance (75%)
Session 3: Learner tries, agent provides hints (50%)
Session 4: Learner tries, agent provides feedback after (25%)
Session 5+: Learner independent, agent monitors (0%)
```

If friction increases, **revert** to higher scaffolding.

---

### 5.5 Review Scheduling (Spaced Repetition)

**Algorithm** (simplified):

```python
def schedule_next_review(skill_id):
    last_practiced = get_last_practiced_date(skill_id)
    mastery = get_mastery_estimate(skill_id)
    difficulty = get_skill_difficulty(skill_id)  # intrinsic complexity

    # Base interval increases with mastery
    if mastery < 0.6:
        interval_days = 1  # practice daily until solidified
    elif mastery < 0.8:
        interval_days = 3
    elif mastery < 0.9:
        interval_days = 7
    else:
        interval_days = 30

    # Adjust for difficulty (harder skills need more frequent review)
    interval_days *= (1 / difficulty)

    next_review = last_practiced + timedelta(days=interval_days)
    return next_review
```

**Trigger**: Daily cron job checks for skills past their review date, generates suggested tasks via nmem's `queue_task` tool.

---

### 5.6 Skill Tree Structure

**Schema** (stored in skill artifacts):

```json
{
  "skill_id": "rust-ownership",
  "name": "Rust Ownership & Borrowing",
  "prerequisites": ["rust-basics", "rust-syntax"],
  "unlocks": ["rust-lifetimes", "rust-smart-pointers"],
  "difficulty": 0.7,
  "dreyfus_stage": "competent",
  "mastery_criteria": {
    "behavioral": ["implement-linked-list-no-errors"],
    "conceptual": ["explain-borrow-checker"],
    "performance": ["debug-ownership-error-under-5min"]
  },
  "exercises": [
    {"id": "kata-linked-list", "type": "implementation"},
    {"id": "kata-borrow-refactor", "type": "refactoring"}
  ]
}
```

**Graph Traversal**: When a skill is mastered:
1. Query graph for `unlocks` field
2. Check if all prerequisites of unlocked skills are now met
3. If yes, add to "available skills" for next session
4. Notify learner: "New skill unlocked: Rust Lifetimes"

---

## Part VI: Implementation Roadmap

### 6.1 Phase 1: Foundation (Current State + Minimal Extensions)

**Already Built (nmem system):**

- ✅ Observation capture (file reads, edits, commands, errors)
- ✅ Episode detection (work units with intent)
- ✅ Session summaries (learned, completed, next steps)
- ✅ 5-dimensional stance classification (phase, scope, locus, novelty, friction)
- ✅ Marker system for tagging sessions (e.g., `tutorial:rust-ownership`)

**Minimal Additions:**

1. **Skill Registry**: Database table or JSON file mapping skill IDs to metadata
   ```sql
   CREATE TABLE skills (
       skill_id TEXT PRIMARY KEY,
       name TEXT,
       prerequisites TEXT,  -- JSON array
       difficulty REAL,
       mastery_criteria TEXT  -- JSON object
   );
   ```

2. **Skill-Session Linkage**: Tag sessions with skills practiced
   ```sql
   CREATE TABLE session_skills (
       session_id TEXT,
       skill_id TEXT,
       mastery_estimate REAL,
       PRIMARY KEY (session_id, skill_id)
   );
   ```

3. **Simple Knowledge Tracing**: After each session, compute mastery estimate per skill
   - **Input**: Episode friction labels, observation errors
   - **Output**: P(mastery) ∈ [0, 1]
   - **Storage**: Update `session_skills.mastery_estimate`

4. **Basic Skill Artifact Generator**: CLI command that queries nmem and generates markdown
   ```bash
   nmem skill-artifact rust-ownership > rust-ownership.md
   ```

**Outcome**: Can track which skills are practiced, estimate mastery, generate skill reports.

---

### 6.2 Phase 2: Adaptive Scaffolding

**Additions:**

1. **Pedagogy Rules Engine**: Simple if-then rules based on mastery estimate
   - **Context Injection Enhancement**: At `SessionStart`, include:
     - Current skill being practiced
     - Mastery estimate
     - Recommended scaffolding level
   - **Example**: "Skill: rust-ownership, Mastery: 0.4, Scaffolding: Provide hints but let learner try first"

2. **Exercise Library**: Pre-defined exercises (katas) per skill
   - **Storage**: JSON or markdown files
   - **Selection**: Algorithm picks exercises matching learner's mastery level

3. **Worked Example Database**: Store successful session transcripts as examples
   - **Retrieval**: When learner is at novice stage, inject worked examples from prior sessions

**Outcome**: System adapts difficulty and support based on learner progress.

---

### 6.3 Phase 3: Skill Tree & Progression Tracking

**Additions:**

1. **Prerequisite Enforcement**: Before starting a session, check if prerequisites are met
   - If not, suggest prerequisite skills

2. **Unlock Notifications**: When skill is mastered, notify learner of newly available skills

3. **Progression Dashboard**: Visualization (could be text-based or web UI)
   ```
   ┌─────────────────────────────────┐
   │ Skill Tree Progress             │
   ├─────────────────────────────────┤
   │ ✓ Rust Basics (Mastered)        │
   │ ✓ Rust Ownership (Mastered)     │
   │ ⚡ Rust Lifetimes (In Progress)  │
   │ 🔒 Rust Async (Locked)           │
   └─────────────────────────────────┘
   ```

**Outcome**: Clear learning path with visible progression.

---

### 6.4 Phase 4: Spaced Repetition & Review

**Additions:**

1. **Review Scheduler**: Cron job that checks for skills needing review
   - Queries `session_skills` for `last_practiced_date`
   - Computes next review date based on mastery + difficulty
   - Generates task via `nmem queue_task`

2. **Retention Tracking**: Measure performance decay
   - Compare current performance (errors, time) to past performance on same exercise
   - If decay detected, flag for review

**Outcome**: Skills remain fresh through spaced practice.

---

### 6.5 Phase 5: Advanced Features

**Additions:**

1. **Deep Knowledge Tracing**: Replace simple BKT with LSTM-based model
   - Train on historical session data
   - Predict mastery with higher accuracy

2. **Generative Skill Artifacts**: Use LLM to generate polished skill documents
   - Input: Structured data from nmem
   - Output: Narrative document with examples, explanations, exercises

3. **Meta-Learning**: Track which pedagogical strategies work for this learner
   - Store in learner profile
   - Apply successful strategies to new skills

4. **Peer Learning**: If multiple learners exist, compare progression
   - "Learner A mastered ownership faster using visual diagrams—try that?"

**Outcome**: Fully automated skill acquisition pipeline.

---

## Part VII: Pitfalls and Mitigations

### 7.1 Over-Reliance on Metrics

**Pitfall**: Optimizing for mastery estimate (P > 0.9) without ensuring deep understanding.

**Mitigation**: Use **multiple signals**:
- Behavioral (can do task)
- Conceptual (can explain)
- Transfer (can apply to novel problems)

Include "teach-back" exercises where learner explains to agent.

---

### 7.2 Premature Optimization

**Pitfall**: Building complex DKT models before validating simple BKT works.

**Mitigation**: **Incremental development**. Start with simple rules, validate, then add complexity. Measure improvement at each step.

---

### 7.3 Ignoring Individual Differences

**Pitfall**: One-size-fits-all pedagogy.

**Mitigation**: **Learner profiles** track preferences:
- Prefers worked examples vs. discovery learning?
- Visual vs. textual explanations?
- Short frequent sessions vs. long deep dives?

Use meta-learning to adapt.

---

### 7.4 Fragile Skill Dependencies

**Pitfall**: Learner "cheats" through prerequisite (low mastery but unlocks next skill).

**Mitigation**: **Strict enforcement** + **reassessment**. If learner struggles with advanced skill, re-check prerequisites. May need to revert and review.

---

### 7.5 Stale Skill Artifacts

**Pitfall**: Skill artifacts become outdated as understanding evolves.

**Mitigation**: **Versioned documents** + **automated regeneration**. Each time skill is practiced, re-run artifact generation to incorporate new learnings.

---

### 7.6 Overfitting to Exercises

**Pitfall**: Learner memorizes kata solutions without understanding.

**Mitigation**: **Varied exercises** + **transfer tests**. Include novel problems that require applying the skill in unfamiliar contexts.

---

### 7.7 Ignoring Affective Factors

**Pitfall**: Focusing only on cognitive progression, ignoring motivation/frustration.

**Mitigation**: Track **session sentiment** (from transcripts or explicit check-ins). If frustration high, reduce difficulty temporarily. If boredom detected, increase challenge.

---

## Part VIII: Success Metrics

### 8.1 Skill-Level Metrics

- **Mastery Estimate**: P(mastery) per skill, tracked over time
- **Time to Mastery**: Sessions from first attempt to mastery threshold
- **Retention**: Performance on skill N weeks after last practice
- **Transfer**: Success on novel tasks requiring the skill

---

### 8.2 Session-Level Metrics

- **Friction Rate**: % of observations with errors/failures
- **Stance Progression**: Movement toward act+converge+routine
- **Learning Velocity**: Skills mastered per week
- **Engagement**: Session frequency, duration

---

### 8.3 System-Level Metrics

- **Completion Rate**: % of learners who master target skills
- **Efficiency**: Avg sessions to mastery (should decrease as system improves)
- **Artifact Quality**: Learner-reported usefulness of skill documents
- **Long-Term Retention**: Performance on skills 6+ months after mastery

---

## Part IX: Integration with nmem

### 9.1 Leveraging Existing Capabilities

**nmem already provides:**

1. **Observation Capture**: All tool use, errors, file touches
2. **Episode Detection**: Work units with intent, friction, hot files
3. **Session Summaries**: Learned, completed, next_steps
4. **Stance Classification**: 5 dimensions per observation
5. **Marker System**: Tag sessions with arbitrary labels (e.g., `tutorial:rust`)
6. **Search & Retrieval**: FTS5 search, file history, recent context
7. **Task Queue**: Schedule future work via `queue_task`

**Dojo builds on this foundation** without reinventing the wheel.

---

### 9.2 New Schema Extensions

**Minimal tables to add:**

```sql
-- Skill definitions
CREATE TABLE skills (
    skill_id TEXT PRIMARY KEY,
    name TEXT,
    description TEXT,
    prerequisites TEXT,  -- JSON array of skill_ids
    difficulty REAL,     -- 0.0 to 1.0
    mastery_criteria TEXT  -- JSON object
);

-- Skill practice tracking
CREATE TABLE session_skills (
    session_id TEXT,
    skill_id TEXT,
    mastery_estimate REAL,  -- P(mastery) after this session
    exercises_attempted TEXT,  -- JSON array
    PRIMARY KEY (session_id, skill_id),
    FOREIGN KEY (session_id) REFERENCES sessions(session_id),
    FOREIGN KEY (skill_id) REFERENCES skills(skill_id)
);

-- Review schedule
CREATE TABLE skill_reviews (
    skill_id TEXT PRIMARY KEY,
    last_practiced_date INTEGER,  -- Unix timestamp
    next_review_date INTEGER,
    review_count INTEGER,
    FOREIGN KEY (skill_id) REFERENCES skills(skill_id)
);
```

---

### 9.3 MCP Tool Extensions

**New tools for the nmem MCP server:**

1. **`get_skill_status`**: Return current mastery estimates for all skills
   ```json
   {
     "rust-ownership": {"mastery": 0.85, "stage": "proficient", "next_review": "2026-04-01"},
     "rust-lifetimes": {"mastery": 0.45, "stage": "advanced_beginner", "next_review": "2026-03-15"}
   }
   ```

2. **`suggest_exercises`**: Given current skill and mastery level, return recommended exercises
   ```json
   {
     "skill_id": "rust-ownership",
     "exercises": [
       {"id": "kata-linked-list", "difficulty": 0.6, "type": "implementation"},
       {"id": "kata-refactor-ownership", "difficulty": 0.5, "type": "refactoring"}
     ]
   }
   ```

3. **`unlock_skills`**: After mastering a skill, check what's newly available
   ```json
   {
     "newly_unlocked": ["rust-lifetimes", "rust-smart-pointers"],
     "still_locked": ["rust-async"]
   }
   ```

4. **`generate_skill_artifact`**: Synthesize skill document from historical sessions
   - Queries observations, episodes, session summaries tagged with the skill
   - Returns markdown document

---

### 9.4 SessionStart Context Enhancement

**Current context injection includes:**

- Intents (recent cross-session patterns)
- Episodes (last 48 hours)
- Session summaries (older sessions)
- Suggested tasks

**Add to context:**

- **Active skill**: "You are currently working on: Rust Ownership (mastery: 0.4, stage: advanced beginner)"
- **Recommended scaffolding**: "Provide hints and partial solutions. Learner is ready to try independently but may need guidance."
- **Available exercises**: "Suggested: kata-linked-list (implementation), kata-borrow-refactor (refactoring)"
- **Prerequisite gaps**: "Warning: rust-basics mastery is only 0.6—may need review before advancing."

This makes the AI agent **aware of pedagogical context** and adapts naturally.

---

### 9.5 Marker Convention Refinement

**Current**: `tutorial:rust-ownership` tags a session.

**Enhanced**:
- `skill:rust-ownership:practice` = practicing the skill
- `skill:rust-ownership:mastered` = skill mastered this session
- `skill:rust-ownership:review` = reviewing after a gap

Markers feed into knowledge tracing and review scheduling.

---

## Part X: Example Workflow

### 10.1 Learner Journey: Rust Ownership

**Session 1: Introduction (Novice)**

1. **Context Injection**: "Starting skill: rust-ownership (mastery: 0.0). Provide full explanations and worked examples."
2. **Agent**: Shows worked example of ownership transfer
3. **Learner**: Reads along, asks clarifying questions
4. **Observations**: High `think+diverge`, `novel`, minimal friction (no code written yet)
5. **Marker**: `skill:rust-ownership:practice`
6. **Summary**: "Learned: basic ownership rules. Next: try implementing simple examples."
7. **Mastery Update**: P(mastery) = 0.1 (exposure but no practice)

---

**Session 2: Guided Practice (Advanced Beginner)**

1. **Context Injection**: "Continuing skill: rust-ownership (mastery: 0.1). Provide hints, let learner try before explaining."
2. **Agent**: Presents exercise: "Implement a function that takes ownership of a String and returns it reversed."
3. **Learner**: Attempts implementation, gets borrow checker error
4. **Observations**: `act+diverge` (trying), `friction` (error)
5. **Agent**: Provides hint: "Remember, after transferring ownership to `reverse()`, the original binding is invalid."
6. **Learner**: Fixes error, succeeds
7. **Marker**: `skill:rust-ownership:practice`
8. **Summary**: "Learned: ownership transfer invalidates original binding. Completed: string reverse exercise."
9. **Mastery Update**: P(mastery) = 0.3 (successful practice with errors)

---

**Session 3-5: Independent Practice (Competent)**

1. **Context Injection**: "Skill: rust-ownership (mastery: 0.5). Provide minimal guidance, let learner solve independently."
2. **Agent**: Presents harder exercise: "Implement a doubly-linked list."
3. **Learner**: Implements, encounters lifetime issues
4. **Observations**: `act+converge` (focused implementation), `friction` (lifetime errors)
5. **Agent**: Delayed feedback after attempt: "Lifetimes are needed here because..."
6. **Learner**: Refactors, succeeds
7. **Marker**: `skill:rust-ownership:practice`
8. **Summary**: "Learned: ownership interacts with lifetimes in complex data structures. Completed: doubly-linked list."
9. **Mastery Update**: P(mastery) = 0.85 (successful with minimal guidance)

---

**Session 6: Mastery Confirmation**

1. **Context Injection**: "Skill: rust-ownership (mastery: 0.85). Challenge problem to confirm mastery."
2. **Agent**: "Refactor this callback-heavy code to use iterator chains."
3. **Learner**: Completes without errors in 12 minutes
4. **Observations**: `act+converge`, `routine`, no friction
5. **Marker**: `skill:rust-ownership:mastered`
6. **Summary**: "Mastered: rust-ownership. Next steps: unlock rust-lifetimes."
7. **Mastery Update**: P(mastery) = 0.95 → **MASTERED**
8. **System**: Unlocks `rust-lifetimes`, schedules first review for 2 weeks

---

**Session 15: Review (Maintenance)**

1. **Triggered by**: Review scheduler detects 2 weeks since last practice
2. **Context Injection**: "Review session for skill: rust-ownership (mastery: 0.95, last practiced 14 days ago)."
3. **Agent**: "Quick check: implement a function that transfers ownership of a Vec."
4. **Learner**: Completes in 5 minutes, no errors
5. **Observations**: `act+converge`, `routine`, no friction
6. **Summary**: "Review: rust-ownership retained."
7. **Mastery Update**: P(mastery) = 0.98 (retention confirmed)
8. **System**: Schedules next review for 1 month

---

### 10.2 Skill Artifact Evolution

**After Session 1:**
```markdown
# Skill: Rust Ownership

## Metadata
- Mastery: 0.1 (Novice)
- Sessions: 1
- Status: In Progress

## Learning Log
- **Session 1**: Introduced to ownership concept. Watched worked example.
```

**After Session 6:**
```markdown
# Skill: Rust Ownership

## Metadata
- Mastery: 0.95 (Mastered)
- Sessions: 6
- Status: Mastered
- Next Review: 2026-03-27

## Learning Progression
### Novice (Session 1)
- Worked examples: ownership transfer, move semantics

### Advanced Beginner (Sessions 2-3)
- Exercises: string reverse, vector manipulation
- Common errors: "value moved here", "borrow after move"

### Competent (Sessions 4-6)
- Projects: doubly-linked list, iterator refactoring
- Breakthroughs: "understood ownership as a compile-time proof system"

## Mastery Evidence
- Completed doubly-linked list implementation
- Refactored callbacks to iterators without errors
- Review session: 100% success rate

## Recommended Exercises
- kata-linked-list: Implement singly-linked list
- kata-ownership-refactor: Refactor clones to borrows

## Pitfalls
- Confusing `String` and `&str`
- Unnecessary `.clone()` calls

## Next Skills
- ✓ Unlocked: Rust Lifetimes
```

This artifact is **generated automatically** from nmem data—no manual writing required.

---

## Part XI: Research-Backed Recommendations

### 11.1 Do's

1. **Use Worked Examples Early**: [Worked example effect](https://en.wikipedia.org/wiki/Worked-example_effect) reduces cognitive load during initial acquisition
2. **Fade Scaffolding Gradually**: [Backward fading](https://cogscisci.wordpress.com/wp-content/uploads/2019/08/sweller-guidance-fading.pdf) optimizes learning efficiency
3. **Space Repetitions**: [74% more effective](https://en.wikipedia.org/wiki/Spaced_repetition) than massed practice
4. **Test Frequently**: [Retrieval practice](https://en.wikipedia.org/wiki/Testing_effect) produces more learning than restudying
5. **Leverage Errors**: [Productive failure](https://pmc.ncbi.nlm.nih.gov/articles/PMC11803059/) leads to deeper understanding
6. **Track Multiple Signals**: Behavioral + conceptual + transfer = robust mastery assessment
7. **Make Progression Visible**: [Belt systems](https://karateintokyo.com/karate_basics/karate-belt-ranking-system-guide/) motivate through visible milestones
8. **Use Prerequisites**: [Skill trees](https://arxiv.org/html/2504.16966v1) ensure foundational competence before advancing
9. **Provide Immediate Feedback**: [Feedback loops](https://pmc.ncbi.nlm.nih.gov/articles/PMC12086178/) improve engagement by 35%
10. **Externalize Knowledge**: [Living documentation](https://www.amazon.com/Living-Documentation-Cyrille-Martraire/dp/0134689321) keeps artifacts current

---

### 11.2 Don'ts

1. **Don't Skip Declarative Stage**: [ACT-R](https://act-r.psy.cmu.edu/about/) shows compilation requires initial factual grounding
2. **Don't Over-Scaffold**: Prolonged support prevents independence
3. **Don't Ignore Individual Differences**: [Meta-learning](https://www.frontiersin.org/journals/education/articles/10.3389/feduc.2025.1697554/full) shows strategies vary per learner
4. **Don't Assume Practice = Expertise**: [Deliberate practice](https://pmc.ncbi.nlm.nih.gov/articles/PMC6731745/) must be effortful and targeted
5. **Don't Neglect Reviews**: [Forgetting curve](https://en.wikipedia.org/wiki/Forgetting_curve) predicts 90% loss without reinforcement
6. **Don't Optimize Metrics Over Understanding**: P(mastery) is a proxy, not the goal
7. **Don't Use High-Fidelity Too Early**: [Flight sims](https://commons.erau.edu/ijaaa/vol5/iss1/6/) show low-fidelity can be better for initial learning
8. **Don't Ignore Affective Factors**: Frustration and boredom derail learning
9. **Don't Hard-Code Pedagogy**: Use adaptive rules that respond to learner state
10. **Don't Let Artifacts Stagnate**: Regenerate as understanding evolves

---

## Part XII: Future Directions

### 12.1 Multi-Agent Learning

**Concept**: Multiple learners working on same skill tree, with peer learning.

**Implementation**:
- Shared skill artifact repository
- Cross-learner comparisons: "Learner B mastered this faster using X strategy"
- Collaborative exercises: pair programming with AI mediating

**Research Basis**: [Bandura's Social Learning Theory](https://www.simplypsychology.org/bandura.html) shows observational learning is powerful.

---

### 12.2 Domain-Specific Dojos

**Concept**: Pre-built Dojo configurations for specific domains.

**Examples**:
- **Rust Dojo**: Skill tree covering ownership → lifetimes → async → unsafe
- **System Design Dojo**: CAP theorem → distributed consensus → eventual consistency
- **Debugging Dojo**: Hypothesis formation → systematic elimination → root cause analysis

**Benefit**: Learners don't start from scratch—leverage community-curated skill trees.

---

### 12.3 Generative Exercises

**Concept**: LLM generates exercises tailored to learner's current mastery level.

**Implementation**:
- **Input**: Skill ID, mastery estimate, recent errors
- **Output**: Novel exercise targeting identified gaps
- **Example**: "Learner struggles with lifetimes in closures → generate exercise involving closure captures with lifetime annotations"

**Research Basis**: [Active Learning](https://en.wikipedia.org/wiki/Active_learning_(machine_learning)) and curriculum learning optimize example selection.

---

### 12.4 Cross-Skill Transfer Detection

**Concept**: Detect when mastery of Skill A accelerates learning of Skill B.

**Implementation**:
- Track "time to mastery" for each skill
- Identify correlations: "Learners who mastered X learned Y 30% faster"
- Suggest optimal learning sequences

**Research Basis**: [Transfer of learning](https://www.digitallearninginstitute.com/blog/unlocking-learning-transfer-through-cognitive-science) is a core cognitive capability.

---

### 12.5 Emotional Intelligence Integration

**Concept**: Track affective state (frustration, boredom, flow) and adapt.

**Implementation**:
- Sentiment analysis on session transcripts
- Explicit check-ins: "How are you feeling about this?"
- Adaptive difficulty: If frustration detected, reduce challenge temporarily

**Research Basis**: [Learning from errors](https://pmc.ncbi.nlm.nih.gov/articles/PMC11803059/) shows motivation management is critical.

---

## Conclusion

The Dojo system synthesizes decades of research into a practical framework for AI-assisted skill acquisition. By layering practice, memory, crystallization, and feedback, it transforms ephemeral learning sessions into **battle-tested, reusable skills**.

**Core Insights:**

1. **Skills progress through stages** (Dreyfus): Scaffold heavily early, fade as competence grows
2. **Knowledge compilation is observable** (ACT-R): Track stance shifts from think+diverge to act+converge
3. **Practice must be deliberate** (Ericsson): Target weaknesses, provide feedback, iterate
4. **Spacing and retrieval trump repetition** (Ebbinghaus, Roediger): Schedule reviews, test don't reread
5. **Errors are learning opportunities** (Kapur): Friction episodes drive understanding
6. **Knowledge must externalize** (Nonaka): Crystallize tacit practice into explicit artifacts
7. **Progression must be visible** (Martial arts): Milestones motivate continued effort
8. **Systems must adapt** (ITS, ZPD): Pedagogy responds to learner state

**The nmem foundation** already provides the memory layer. The Dojo builds on this with skill trees, knowledge tracing, adaptive scaffolding, and artifact generation. The result: a system that **learns how its learners learn**, improving over time through meta-learning.

From "I want to learn Rust" to "here's a battle-tested Rust ownership skill"—the Dojo makes that journey systematic, measurable, and repeatable.

---

## Sources

- [Dreyfus Model of Skill Acquisition](https://en.wikipedia.org/wiki/Dreyfus_model_of_skill_acquisition)
- [Dreyfus Model Critical Perspective](https://pmc.ncbi.nlm.nih.gov/articles/PMC2887319/)
- [Deliberate Practice Framework](https://pmc.ncbi.nlm.nih.gov/articles/PMC6731745/)
- [Deliberate Practice Debate](https://pmc.ncbi.nlm.nih.gov/articles/PMC7461852/)
- [Mastery Learning](https://en.wikipedia.org/wiki/Mastery_learning)
- [Bloom's Taxonomy](https://www.simplypsychology.org/blooms-taxonomy.html)
- [Zone of Proximal Development](https://www.simplypsychology.org/zone-of-proximal-development.html)
- [ITS and ZPD](https://link.springer.com/chapter/10.1007/3-540-47987-2_75)
- [Worked Example Effect](https://en.wikipedia.org/wiki/Worked-example_effect)
- [Guidance Fading Effect](https://cogscisci.wordpress.com/wp-content/uploads/2019/08/sweller-guidance-fading.pdf)
- [Cognitive Load Theory](https://ppig.org/files/2003-PPIG-15th-shaffer.pdf)
- [Spaced Repetition](https://en.wikipedia.org/wiki/Spaced_repetition)
- [Forgetting Curve](https://en.wikipedia.org/wiki/Forgetting_curve)
- [Testing Effect](https://en.wikipedia.org/wiki/Testing_effect)
- [Active Retrieval](https://learninglab.psych.purdue.edu/downloads/2012/2012_Karpicke_CDPS.pdf)
- [Learning from Errors](https://pmc.ncbi.nlm.nih.gov/articles/PMC11803059/)
- [Productive Failure](https://bpspsychub.onlinelibrary.wiley.com/doi/10.1111/bjep.12716)
- [Meta-Learning and Cognitive Mirror](https://www.frontiersin.org/journals/education/articles/10.3389/feduc.2025.1697554/full)
- [Cognitive Apprenticeship](https://www.aft.org/ae/winter1991/collins_brown_holum)
- [Code Kata](http://codekata.com/)
- [Coding Dojo](https://codingdojo.org/kata/)
- [Apprenticeship Patterns](https://www.oreilly.com/library/view/apprenticeship-patterns/9780596806842/)
- [Flight Simulator Fidelity](https://commons.erau.edu/ijaaa/vol5/iss1/6/)
- [Karate Belt System](https://karateintokyo.com/karate_basics/karate-belt-ranking-system-guide/)
- [Bayesian Knowledge Tracing](https://en.wikipedia.org/wiki/Bayesian_Knowledge_Tracing)
- [Deep Knowledge Tracing](https://stanford.edu/~cpiech/bio/papers/deepKnowledgeTracing.pdf)
- [Learning Analytics and EDM](https://files.eric.ed.gov/fulltext/ED611199.pdf)
- [Skill Trees](https://arxiv.org/html/2504.16966v1)
- [Knowledge Crystallization Cycle](https://arxiv.org/html/2603.10808)
- [SECI Model](https://ascnhighered.org/ASCN/change_theories/collection/seci.html)
- [Living Documentation](https://www.amazon.com/Living-Documentation-Cyrille-Martraire/dp/0134689321)
- [Docs-as-Code](https://konghq.com/blog/learning-center/what-is-docs-as-code)
- [Adaptive Feedback Systems](https://pmc.ncbi.nlm.nih.gov/articles/PMC12086178/)
- [Active Learning (ML)](https://en.wikipedia.org/wiki/Active_learning_(machine_learning))
- [ACT-R Cognitive Architecture](https://act-r.psy.cmu.edu/about/)
- [Production Compilation](https://pubmed.ncbi.nlm.nih.gov/12916582/)
- [Chunking](https://en.wikipedia.org/wiki/Chunking_(psychology))
- [Bandura's Social Learning Theory](https://www.simplypsychology.org/bandura.html)
- [Transfer of Learning](https://www.digitallearninginstitute.com/blog/unlocking-learning-transfer-through-cognitive-science)
