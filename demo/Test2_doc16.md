# The Swift Desktop Shell

## Responsibilities

The Swift/SwiftUI application (OxcerLauncher) has three responsibilities: hosting the local LLM, driving the agent step loop via the FFI boundary, and owning all user-facing UI. It deliberately contains no business logic — routing, plan building, and tool selection all happen in Rust. The Swift layer is a thin driver: pass task in, receive intent, execute intent, pass result back, repeat.

## MVVM Architecture

The UI follows a standard MVVM pattern. `AppViewModel` is an `@MainActor ObservableObject` that owns the chat message list, the running-task flag, the pending-approval bubble, and the `AgentRunner` instance for the current request. `ContentView` observes `AppViewModel` and renders the chat view, the input field, the stop button, and the approval bubble. `DetailView` and `SettingsView` are separate sheets.

## AgentRunner and SwiftAgentExecutor

The step loop is factored into two structs. `AgentRunner` drives the `ffi_agent_step` loop, manages the `maxSteps` counter, and handles approval gating. `SwiftAgentExecutor` handles the actual execution of each `FfiToolIntent` kind: LLM generation, file operations, and shell commands. This separation means the loop logic is testable without a live backend, and the executor is testable without a running step loop.

## The FFI Boundary

Oxcer uses UniFFI 0.28 with attribute-based macros (no `.udl` file). The generated Swift bindings live in `apps/OxcerLauncher/OxcerLauncher/oxcer_ffi.swift` and the C header in `oxcer_ffiFFI.h`. The release dylib is built with `cargo build --release -p oxcer_ffi` and linked by Xcode. After any change to a `#[uniffi::export]` function, both files must be regenerated with `scripts/regen-ffi.sh` and committed together with the Rust source change.

## generateText and the LLM Call

`OxcerLauncher.generateText(prompt:)` is the sole entry point for on-device LLM inference from Swift. It is an `async throws` function backed by a Rust `spawn_blocking` thread that runs the `llama-cpp-2` inference loop. The `SwiftAgentExecutor` wraps this call in a `withThrowingTaskGroup` timeout race so a stalled model does not block the UI indefinitely.
