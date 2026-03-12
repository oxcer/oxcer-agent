# The content_accumulator Pattern

## The Problem It Solves

A single `accumulated_response` field is sufficient when a workflow reads one file and passes its content to the LLM. But for a 20-file summarise workflow, the content of each file must be preserved until all reads are complete. Overwriting `accumulated_response` on each read would lose the previous files' content. The `content_accumulator` is a `Vec<String>` that grows with every successful `FsReadFile` result without discarding earlier entries.

## Accumulation Logic

In `next_action`, after processing a `StepResult::Ok` from a step whose plan entry is `FsReadFile`, the orchestrator pushes the `text` field of the payload into `session.content_accumulator`. This happens before the `step_index` is incremented, so the accumulator always reflects results up to and including the step that just completed.

## Joining the Contents

When the orchestrator emits a `LlmGenerate` step whose task string contains the `{{FILE_CONTENTS}}` placeholder, it joins the accumulator entries with `\n\n---\n\n` before substituting. The `---` separator is a Markdown horizontal rule, which is recognisable to any model trained on Markdown and clearly delineates file boundaries in the prompt.

## Fallback Behaviour

If `content_accumulator` is empty when `{{FILE_CONTENTS}}` substitution occurs — which should not happen in a correctly constructed plan but is handled defensively — the orchestrator falls back to `accumulated_response` (the last single-tool text result). If that is also empty, the placeholder is replaced with `"(no content)"`. This ensures the LLM always receives a valid prompt string, even in edge cases.

## No Clearing Between Steps

`content_accumulator` is never cleared during a session. In the current design, each session handles a single user request, so there is no risk of accumulating content from a previous request. If multi-request sessions are added in a future version, the accumulator will need to be reset at session start.
