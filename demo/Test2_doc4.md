# Plan Builders

## The Role of Plan Builders

Plan builders are pure functions in `orchestrator.rs` that take a task string, a workspace ID, a workspace root, and a strategy, and return a `Vec<ToolCallIntent>`. They are called exactly once, at session start, after the router has determined which builder is appropriate. Once the plan is returned, the orchestrator stores it in `SessionState` and never calls the builder again.

## Static vs Sentinel Plans

Some plan builders produce a fully resolved plan — every intent is known upfront. `build_plan_file_read_then_llm` is an example: it produces `[FsReadFile(path), LlmGenerate(summarise)]` with the real path already filled in. Other builders produce a sentinel plan — a shorter plan that contains placeholders or relies on a `pending_expansion` to insert steps after a directory listing arrives.

## The Sentinel Pattern

A sentinel plan for multi-file summarise looks like `[FsListDir(dir), LlmGenerate({{FILE_CONTENTS}})]`. The `LlmGenerate` step cannot be executed yet because the file list is unknown. After `FsListDir` succeeds, `do_expand_plan` splices in N×`FsReadFile` steps between the two existing steps. The final `LlmGenerate` then receives all accumulated text via the `{{FILE_CONTENTS}}` substitution.

## Prompt Construction

Plan builders are also responsible for constructing the LLM prompt that will be passed to `LlmGenerate`. The prompt includes the original task description (sanitised to remove any control characters), instructions to base the answer solely on tool results, and one or more placeholders (`{{FS_RESULT}}` or `{{FILE_CONTENTS}}`) that will be replaced with real data before the intent is emitted. This prevents the model from being asked to answer a question it has not yet been given the data for.

## Choosing the Right Builder

The `start_session` function in `orchestrator.rs` is a large match expression that maps routing decisions to plan builders. The arms are ordered most-specific to least-specific to ensure that precise patterns (single named file in a known directory, multi-file move, most-recent file) are tried before the generic fallbacks (implicit FS intent, implicit file read intent, full LLM plan).
