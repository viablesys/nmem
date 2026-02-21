# ADR-012: Visualization Server — Bespoke Design Analysis Tool

## Status
Accepted

## Framing
*How does an agent-driven design session present structural visualizations (dependency graphs, data flow diagrams, performance plots) in real-time, without inline terminal images, without cloud services, and without a build toolchain?*

## Depends On
- ADR-010 (Work Unit Detection) — episode narratives are a primary source of visualization content
- ADR-011 (Phase Classification) — phase distribution is a natural plotly viz

## Unlocks
- Real-time visual feedback during design analysis sessions
- Iterative diagram refinement (POST, observe, PUT update, repeat)
- Portable artifact metadata — same mermaid source renders in viz UI, GitHub, Obsidian
- Foundation for interactive/dynamic visualizations beyond static files

---

## Context

### The problem

Claude Code sessions regularly produce structural analysis — module dependency graphs, data flow diagrams, performance comparisons, schema relationships. These are generated as text (mermaid source, plotly specs, dot notation) but there's no rendering surface. The terminal can't display images. Copy-pasting to an external tool breaks the flow. The analysis happens but the artifact is lost.

### Why not existing tools?

- **Static file generation** (save SVG to disk, open in browser) — works once, no iteration loop, no live updates, no metadata
- **Jupyter notebooks** — heavy, requires kernel, wrong abstraction for push-from-CLI workflow
- **Grafana** — already running, but dashboards are for metrics, not ad-hoc design diagrams
- **Web IDE preview** — tied to specific editors, not available in terminal-only sessions

### What we actually need

A local HTTP server that:
1. Accepts viz content via POST (curl from CLI, no browser interaction required)
2. Renders immediately in a browser tab via SSE (no refresh)
3. Supports multiple viz types (diagrams, charts, markdown, raw SVG)
4. Tags artifacts with caller-supplied metadata (generator, version, session, project)
5. Persists across browser refreshes but treats artifacts as ephemeral (JSON file, not a database)
6. Has zero install dependencies (Python stdlib server, CDN-loaded browser libs)

## Decision

### Bespoke local viz server at `~/workspace/viz/`

Three files:
- `viz.py` — Python stdlib `http.server` + `threading` for SSE, JSON persistence
- `index.html` — Tailwind dark UI, client-side rendering via CDN libs
- `viz.json` — auto-created, gitignored artifact store

Port 37778 (env override: `VIZ_PORT`).

### Architecture choices

**Python stdlib, no framework.** The server is ~200 lines. Flask/FastAPI would add dependency management for zero benefit. The threading model (one thread per SSE connection) is adequate for 1-3 browser tabs.

**Client-side rendering.** Mermaid, Plotly, and marked.js run in the browser via CDN. The server never invokes `dot`, `mmdc`, or any rendering binary. This eliminates server-side dependencies entirely.

**Mermaid over Graphviz for diagrams.** Graphviz is a layout engine — you describe topology and it decides positions. For curated architecture diagrams where layer ordering matters, this is the wrong abstraction. Mermaid's flowchart renderer respects subgraph declaration order and produces predictable top-down layouts. Bonus: mermaid source is valid in markdown fences, so the same content renders in GitHub, Obsidian, and the viz UI.

**Caller-controlled metadata.** The server doesn't impose structure on metadata — it's an arbitrary key-value dict. Common conventions (`generator`, `version`, `session`, `project`) are documented but not enforced. Multiple artifacts with the same title coexist; each has a unique `id` plus whatever metadata the caller provides.

**PUT for updates, not POST-to-replace.** An artifact can be iteratively refined without changing its ID. The caller captures the ID from the POST response and PUTs updates. Metadata merges (add/overwrite keys without losing existing ones). This supports the "POST, observe, refine, PUT" loop.

### What this is not (yet)

This is a static-content viewer with a push API. The viz types today are all "here's content, render it." The architecture deliberately leaves room for:

- **Interactive vizs** — a `type: "app"` that loads a JS module with callbacks, not just static markup
- **Bidirectional flow** — browser actions (click a node, drag a boundary) that POST events back to the server, enabling agent-in-the-loop exploration
- **Live data binding** — plotly charts that poll or subscribe to a data endpoint, not just render a snapshot
- **Composable layouts** — multiple vizs arranged in a dashboard-like grid, not just a vertical feed
- **Export** — render to PNG/PDF for inclusion in documents

None of these require architectural changes to the server. The SSE infrastructure, metadata model, and type-dispatched rendering are all extensible. The current implementation is the minimal useful tool; iteration will be driven by actual usage patterns.

## Consequences

### Positive
- Zero-dependency local tool, works anywhere Python 3.12+ is installed
- Sub-second feedback loop: POST → browser renders via SSE
- Mermaid source is portable to any markdown renderer
- Metadata makes artifacts distinguishable even with duplicate titles
- Bespoke tool means we control the iteration pace — no upstream breaking changes

### Negative
- Another local service to start (mitigated: single `python3 viz.py` command)
- CDN dependency for browser libs (mitigated: works offline after first load due to browser cache)
- No authentication — localhost-only is the security model

### Risks
- Scope creep into a general-purpose dashboard tool (mitigated: viz is a design analysis tool, Grafana exists for operational metrics)
- Mermaid layout limitations for very large graphs (mitigated: can fall back to SVG type for manual layout)

## References
- Repository: `github.com/viablesys/viz` (private)
- Server: `viz.py` v0.3.0
- Renderers: Mermaid 11.x, Plotly 2.35, marked 15.x, @hpcc-js/wasm-graphviz 1.6 (all CDN)
