# Scope Classification: Converge vs Diverge

You are labeling text observations from a coding assistant's session for a binary classifier.

## Definitions

**diverge** — The observation broadens the search space. The agent is exploring, scanning, surveying, or considering multiple options. Examples:
- Reading files to understand a codebase ("what does this do?")
- Searching with broad glob or grep patterns
- Web searching for options, libraries, or approaches
- Reading multiple files across different directories
- Exploring new or unfamiliar code
- Reading documentation to learn about a feature
- Investigating how something works
- Asking questions about requirements or design
- Looking at multiple files to understand a pattern
- Initial research before deciding on an approach

**converge** — The observation narrows toward a solution. The agent is focused on a specific target, finishing, or committing. Examples:
- Editing a specific file to fix a known bug
- Writing a targeted implementation
- Committing changes (git commit, git push)
- Re-running a specific test after a fix
- Making a specific, targeted edit to one location
- Writing or updating a specific function
- Pushing completed work
- Running a build after making changes
- Targeted read of a specific file to verify a fix
- Applying a known solution to a known problem

## Key Distinction

The question is: **Is this observation expanding possibilities or narrowing to a conclusion?**

- Reading a file you've never seen → **diverge** (exploring)
- Reading a file you just edited to verify → **converge** (verifying)
- Searching "*.rs" across the whole project → **diverge** (scanning)
- Searching for a specific error message → **converge** (targeting)
- Web searching "best practices for X" → **diverge** (researching options)
- Editing a file to fix the specific bug → **converge** (implementing)
- Running tests to see what breaks → **diverge** (discovering)
- Running tests after fixing a specific test → **converge** (verifying)

## Output Format

For each observation, output:
```json
{"id": <id>, "type": "converge"|"diverge"}
```

Label the ENTIRE batch as a JSON array.
