# The Semantic Router

## What the Router Does

Before any plan is built, every incoming task passes through the semantic router in `oxcer-core/src/semantic_router.rs`. The router classifies the task into one of three strategies: `ToolsOnly` (direct tool execution with no LLM step), `CheapModel` (local Llama 3, the default), or `ExpensiveModel` (reserved for a future cloud path requiring deeper reasoning).

## Rule-Based Classification

The router is intentionally rule-based rather than model-driven. Using a model to decide which model to call adds latency, consumes context, and can fail in ways that are hard to reason about. A set of keyword and pattern tests — checking for explicit file paths, known directory names, action verbs like "move" or "delete", and digit counts suggesting multi-file operations — is fast, deterministic, and easy to extend.

## The ToolsOnly Path

When `prefer_tools_only` is set in the router config (used for direct commands like "delete foo.txt" or "list workspace"), the router emits a `ToolsOnly` strategy. The orchestrator then builds a plan consisting of a single tool call with no LLM step at all. This path has zero model latency and is used for unambiguous imperative commands.

## Routing Decisions That Affect Plan Shape

Several routing decisions directly determine the structure of the plan. Detecting a bare filename with a known extension alongside a directory keyword (e.g. "Test1_doc.md in Downloads") causes the orchestrator to read that file directly rather than listing the directory first. Detecting a digit plus "files/reports" alongside a summarise verb triggers a multi-file sentinel plan. Detecting "move … into … folder called" triggers the create-and-move expansion path.

## Keeping the Router Lean

The router deliberately does not parse natural language deeply. It does not attempt to resolve pronouns or track conversation history. Its job is to answer one question — which plan builder should handle this task? — and pass everything else through to the orchestrator untouched. This keeps it fast, testable, and straightforward to extend as new workflow patterns are added.
