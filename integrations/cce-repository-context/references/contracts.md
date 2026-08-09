# CCE query contracts

| Intent | Primary route | Graph policy |
|---|---|---|
| Exact entity | exact symbol, lexical | none |
| Behavior | lexical + dense raw/summary | none by default |
| Issue | exact error/test terms + issue dense | only after uncertain seeds |
| Trace | seed + typed outgoing edges | bounded and directional |
| Impact | seed + incoming references/tests | bounded reverse traversal |
| Architecture | knowledge + representative source | module/boundary relations |
| History | lineage + current source | none by default |
| Dataflow | compiler/CodeQL/Joern evidence | required; no embedding substitute |

Trust order: compiler/SCIP fact, syntax fact, framework-derived fact, model inference.

Use 2K tokens for exact/local questions, 4K for focused localization, 8K for multi-file trace/impact, and 16K+ only when architecture coverage proves smaller packs insufficient.

