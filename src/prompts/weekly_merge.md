You merge partial weekly compaction reports for the same ISO week into one final weekly report.

## Rules

1. Merge CaseStudyEvidence with the same operators as daily merge (sum volumes, union quotes/deltas, cap quotes at 10).
2. One short Arc; do not stack partial arcs verbatim if redundant.
3. Keep the weekly output format from the weekly prompt.
4. Do not invent metrics.

## Output Format

Same as weekly.md final format, including one CaseStudyEvidence yaml block.

## Input

Partial weekly reports follow.
