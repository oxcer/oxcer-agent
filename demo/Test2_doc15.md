# Safety Gates and the Approval UI

## The Principle

Oxcer never executes a filesystem or shell tool without explicit user consent. This is not an optional feature — it is a hard constraint enforced in `AgentRunner.swift` before the `SwiftAgentExecutor` is called. The set of tools requiring approval includes every operation that touches the filesystem (`fs_list_dir`, `fs_read_file`, `fs_write_file`, `fs_delete`, `fs_rename`, `fs_move`, `fs_create_dir`) and shell execution (`shell_run`). Only `llm_generate` is exempt, because it is pure computation with no side effects on disk.

## The Approval Gate in AgentRunner

When the orchestrator emits a `NeedTool` outcome with an intent kind in `approvalRequiredKinds`, `AgentRunner` pauses the step loop and calls `onApprovalNeeded(requestId, summary)`. This is an `async` closure that suspends the loop — releasing the main actor for UI events — until the user responds. On approval, execution continues. On denial, a `FfiStepResult(ok: false, error: "User denied: ...")` is fed back to the orchestrator so it can surface the denial in the final answer.

## The ApprovalRequest Type

In SwiftUI, the pending approval is represented as an `ApprovalRequest: Identifiable, Sendable` value stored in `AppViewModel.pendingApproval`. It wraps a `CheckedContinuation<Bool, Never>` that is resumed when the user taps a button. The `approve()` and `cancel()` methods on `ApprovalRequest` resume the continuation with `true` or `false` respectively, and the continuation is then consumed exactly once.

## The Approval Bubble UI

The approval bubble appears inline in the chat view. It shows a lock-and-shield icon, the plain-English summary of the requested action, a keyboard hint ("↩ Return · ⎋ Escape"), and Approve / Cancel buttons. The summary is constructed by `AgentRunner.approvalSummary(for:)` which decodes the intent JSON and produces a sentence like "Allow Oxcer to list files under: ~/Downloads" or "Allow Oxcer to create folder: ~/Desktop/Test_folder."

## Forward Compatibility

The `awaiting_approval` status in the step loop's switch statement is a forward-compatibility path for Rust-driven approval requests (e.g. from a future security policy engine). In v0.1.0 the Swift-side gate always fires before the executor runs, so `awaiting_approval` is never reached in practice. It is present so that adding a Rust-side policy engine in a future version does not require changes to the step loop switch.
