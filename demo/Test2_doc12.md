# Dynamic Plan Expansion

## The Core Idea

Some workflows cannot be fully planned before the agent has seen the contents of a directory. The number of files to read or move is unknown until `FsListDir` returns. Dynamic plan expansion is the mechanism by which the orchestrator inserts the correct number of concrete steps into the live plan after the listing arrives, without rebuilding the plan from scratch.

## The pending_expansion Field

`SessionState` carries a `pending_expansion: Option<ExpansionKind>` field. It is set during `start_session` when a sentinel plan is built, and it is consumed — taken, not cloned — the first time the orchestrator detects that a `FsListDir` step has just completed. Taking rather than cloning ensures the expansion fires exactly once per session, preventing accidental repeated expansion.

## ExpansionKind

`ExpansionKind` is a Rust enum with two variants. `ReadAndSummarize { file_filter }` causes `do_expand_plan` to insert N×`FsReadFile` steps for the readable files in the listing. `MoveToDir { dest_workspace_id, dest_workspace_root, dest_rel_dir, file_filter }` causes it to insert one `FsCreateDir` step followed by N×`FsMove` steps. The `file_filter` field, if present, limits expansion to files whose names contain the filter string.

## Vec::splice

The insertion is performed with Rust's `Vec::splice` method, which removes a range of elements and inserts an iterator of replacements at the same position. This allows the orchestrator to insert N new steps before the sentinel `LlmGenerate` step without allocating a new vector. After splicing, the plan vector contains the original steps plus the dynamically determined ones, and the `step_index` still correctly points to the next step to execute.

## Timing

The expansion check runs at the top of the "More steps?" section of `next_action`, after the latest result has been applied and the `step_index` incremented. This guarantees that by the time the expansion fires, `session.confirmed_root` and `session.last_dir_listing_sorted` have already been populated from the `FsListDir` payload.
