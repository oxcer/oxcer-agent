# Oxcer: A Local-First Desktop AI Agent

## Philosophy

Oxcer is built on a single conviction: AI assistance should not require sending your files to the cloud. Every document you ask about, every folder you ask to organise, every report you want summarised stays on your machine. The model runs locally; the files never leave your disk.

This local-first stance is not just a privacy feature — it is a design constraint that shapes everything. Because the agent cannot rely on a server-side conversation history or a managed execution environment, it must be explicit about state, plan every action in advance, and surface every non-trivial operation to the user for approval before executing it.

## Privacy and the Guard-Rail Model

Oxcer follows a guard-railed agent architecture. Before any filesystem or shell tool runs, the user sees a plain-English description of what the agent is about to do and must approve it. There is no silent background operation. A Swift approval bubble appears in the chat UI; the step loop pauses until the user taps Approve or Cancel.

This design deliberately avoids the "just let the AI do it" pattern that makes power users comfortable but non-developer users anxious. The approval prompt is short, specific, and always shows the actual path or command — never a vague "the agent wants to access a file."

## Architecture Overview

The system is split into two layers. The core logic lives in a Rust crate called `oxcer-core`, which implements a finite-state orchestrator, a semantic router, and all tool-call intent types. The desktop shell is a native Swift/SwiftUI macOS application that hosts the local LLM, drives the step loop via a Foreign Function Interface, and owns all user-facing UI including the approval gate.

The Rust layer is intentionally stateless from the Swift side's perspective: it receives a serialised `SessionState` blob on each step, updates it, and returns the next `ToolCallIntent` plus the new session blob. Swift stores the blob opaquely and passes it back unchanged. This makes the orchestrator easy to test in isolation without a running UI.

## The Filesystem Tool Suite

Oxcer exposes six filesystem operations as first-class agent tools: `FsListDir` lists a directory, `FsReadFile` reads a text file, `FsWriteFile` creates or overwrites a file atomically, `FsDelete` removes a file, `FsMove` relocates a file across directories, and `FsCreateDir` creates a folder tree with intermediate directories. Each tool is identified by a short string kind (`fs_list_dir`, `fs_read_file`, etc.) that travels through the FFI boundary and is dispatched by the Swift executor.

## The Local LLM Backend

The on-device language model is Meta Llama 3 8B Instruct, quantised to Q4_K_M (~4.9 GiB) and run via `llama-cpp-2` with full Metal GPU offload on Apple Silicon. Generation uses the Llama 3 chat template with a system prompt that instructs the model to use tools rather than fabricate answers. A Swift concurrency timeout (configurable, default 120 s) prevents the step loop from hanging if the model stalls.

## Non-Developer UX

Oxcer is aimed at people who would describe themselves as "not technical." The chat interface uses plain prose, not slash commands. The approval bubble shows a sentence like "Allow Oxcer to list files under: ~/Downloads" rather than a JSON payload. Error messages describe what went wrong in terms of the user's intent, not the underlying system call. The three v0.1.0 demo workflows — single-file summary, multi-file overview, and folder-to-folder move — are expressible in natural language with no special syntax.
