# OxcerLauncher: Global Memory Analysis & Structural Improvement Plan

This document lists **every** suspicious memory-growth site in the OxcerLauncher codebase (Swift + FFI boundary + Rust), explains how memory can grow, classifies lifetime and boundedness, then proposes structural improvements and concrete refactors per site.

**Prior mitigations (already in place):** Capped chat messages (e.g. 500) and trim on append; limited session events (e.g. 2000) in UI; cleared session events on "New Chat"; throttled download progress on Swift side; Shimmer and overlay animations tied to view/scenePhase; session-lifetime refactor so only one `SessionDetailViewModel` holds messages and sessionEvents and AppViewModel holds lightweight `[SidebarSessionItem]` only. **Implemented:** (1) Rust telemetry caps: `load_session_log_from_dir` returns at most 2000 events (tail), `list_sessions_from_dir` skips files > 50 MB and reads head+tail for large files; (2) Swift `runAgentRequest` no longer calls `loadSessions()` on every response — only on initial load, explicit Refresh, and once after first response in a new chat; (3) `maxSessionsCount` aligned to Rust cap (100).

---

# Part 1: Suspicious Memory Growth Patterns (Full List)

---

## 1. Rust: `load_session_log_from_dir` — unbounded load of session log

**File:** `oxcer-core/src/telemetry.rs` (lines 298–313)

```rust
pub fn load_session_log_from_dir(...) -> Result<Vec<LogEvent>, String> {
    let content = std::fs::read_to_string(&path).map_err(...)?;  // ENTIRE FILE
    let mut events = Vec::new();
    for line in content.lines().filter(|s| !s.is_empty()) {
        if let Ok(ev) = serde_json::from_str::<LogEvent>(line) {
            events.push(ev);
        }
    }
    Ok(events)  // UNBOUNDED Vec
}
```

- **How memory grows:** One `String` = full file (e.g. 10 MB–1 GB). Then a `Vec<LogEvent>` with one entry per line; each `LogEvent` has `details: serde_json::Value` (can be large JSON). No cap on line count.
- **Lifetime:** Request (call) lifetime; but the *size* of the request is unbounded.
- **Bounded?** **No.** File size and event count are unbounded. Selecting a session with a huge log can allocate hundreds of MB or more in one go.

---

## 2. Rust -> FFI: `load_session_log` returns full `Vec<LogEvent>`

**File:** `oxcer_ffi/src/lib.rs` (lines 454–459)

```rust
pub fn load_session_log(...) -> Result<Vec<LogEvent>, OxcerError> {
    let events: Vec<CoreLogEvent> = load_session_log_from_dir(&dir, &session_id)?;
    Ok(events.iter().map(log_event_to_ffi).collect())  // full copy, no cap
}
```

- **How memory grows:** Full `Vec<LogEvent>` is serialized into one RustBuffer and sent to Swift. No truncation; Rust sends everything.
- **Lifetime:** Request.
- **Bounded?** **No.** Same unbounded size as (1). Plus a second full copy in FFI serialization.

---

## 3. Swift: FFI sequence lift builds full `[LogEvent]` before trim

**File:** `apps/OxcerLauncher/OxcerLauncher/oxcer_ffi.swift` (FfiConverterSequenceTypeLogEvent, lines 1519–1526)

```swift
public static func read(from buf: inout (data: Data, offset: Data.Index)) throws -> [LogEvent] {
    let len: Int32 = try readInt(&buf)
    var seq = [LogEvent]()
    seq.reserveCapacity(Int(len))
    for _ in 0 ..< len {
        seq.append(try FfiConverterTypeLogEvent.read(from: &buf))
    }
    return seq
}
```

- **How memory grows:** Decodes **every** event from the buffer. If Rust sent 500k events, Swift allocates 500k `LogEvent` (each with multiple `String`s, including `details`). Only *after* this does `SessionDetailViewModel` do `Array(events.prefix(2000))`.
- **Lifetime:** Request (temporary), but peak = full decoded array.
- **Bounded?** **No.** Cap is applied only after the full array exists; peak memory can be tens of GB for a large log file.

---

## 4. Rust: `list_sessions_from_dir` reads every session file in full

**File:** `oxcer-core/src/telemetry.rs` (lines 207–294)

```rust
for entry in dir_entries {
    ...
    let content = match std::fs::read_to_string(&path) { ... };  // FULL FILE per session
    let lines: Vec<&str> = content.lines().filter(...).collect();
    let mut events = Vec::with_capacity(lines.len());
    for line in &lines {
        if let Ok(ev) = serde_json::from_str::<LogEvent>(line) {
            events.push(ev);
        }
    }
    // ... build SessionSummary from events ...
    summaries.push(SessionSummary { ... });
}
summaries.truncate(100);
```

- **How memory grows:** For **each** session file: one full `String` (entire file) + full `Vec<LogEvent>` for that file. One 500 MB session file -> ~500 MB + parsed events. With 100 sessions we iterate 100 times; we don’t hold all at once, but each iteration can be huge.
- **Lifetime:** Request (one call to list_sessions).
- **Bounded?** **Partially.** Output is capped at 100 summaries, but **input** per file is unbounded. One giant session file can still cause a multi‑GB spike.

---

## 5. Swift: `loadSessions()` after every agent response

**File:** `apps/OxcerLauncher/OxcerLauncher/ContentView.swift` (runAgentRequest, lines 422–434)

```swift
do {
    let response = try await backend.runAgentTask(payload: payload)
    // ...
    await loadSessions()  // re-loads sessions list every time
}
```

- **How memory grows:** Every agent reply triggers `loadSessions()` -> Rust `list_sessions_from_dir` -> full read of every session file again. Long chat = many responses = repeated full scans and allocations.
- **Lifetime:** Request (per response), but **frequency** is high.
- **Bounded?** **No.** Single call can be huge (see 4); and it’s invoked repeatedly.

---

## 6. Rust: download progress callback — many allocations per chunk

**File:** `oxcer-core/src/llm/downloader.rs` (lines 61–86)

```rust
while let Some(chunk) = stream.next().await {
    // ...
    callback_clone.on_progress(
        progress,
        format!("Downloading... {}%", pct),  // new String every chunk
    );
}
```

- **How memory grows:** For a 2.4 GB file with ~64 KB chunks, ~40k callbacks. Each `format!(...)` allocates a new `String`. Each callback crosses FFI and can trigger Swift-side work (throttled now, but still many Rust-side allocations).
- **Lifetime:** Request (duration of download).
- **Bounded?** **Yes** (download ends). Impact: high allocation rate and possible pressure, not unbounded growth.

---

## 7. Swift: `RustBuffer(bytes: writer)` — full copy when lowering

**File:** `oxcer_ffi.swift` (FfiConverterRustBuffer.lower, lines 209–212; RustBuffer.from, 27–28)

```swift
return RustBuffer(bytes: writer)  // writer is [UInt8], full copy
// RustBuffer.from allocates new buffer via ffi_oxcer_ffi_rustbuffer_from_bytes
```

- **How memory grows:** Every `lower()` of a large value (e.g. `AgentRequestPayload` with large `task_description`, or `[LogEvent]`) builds a `[UInt8]` writer then copies it into a RustBuffer. Two full copies (Swift array + Rust buffer).
- **Lifetime:** Request.
- **Bounded?** Depends on payload. For `load_session_log` return value, buffer size = serialized full `[LogEvent]` -> unbounded (same as 1–3).

---

## 8. Swift: `Data(rustBuffer:)` with `deallocator: .none`

**File:** `oxcer_ffi.swift` (lines 51–58)

```swift
fileprivate extension Data {
    init(rustBuffer: RustBuffer) {
        self.init(
            bytesNoCopy: rustBuffer.data!,
            count: Int(rustBuffer.len),
            deallocator: .none  // Swift does NOT own; Rust must free
        )
    }
}
```

- **How memory grows:** Data does not copy; it’s a view. But `lift()` then **reads** from this Data and builds Swift types (e.g. full `[LogEvent]`), so we still get a full decoded copy. The buffer is freed in `lift()` after read. So we don’t double-own, but we do have one full decoded copy.
- **Lifetime:** Request.
- **Bounded?** Same as (3): decoding is unbounded.

---

## 9. Swift: `sessions` and `SidebarSessionItem` mapping

**File:** `ContentView.swift` (loadSessions, line 398); `SessionModels.swift` (SidebarSessionItem.from)

```swift
sessions = Array(list.prefix(maxSessionsCount).map { SidebarSessionItem.from($0) })
```

- **How memory grows:** `list` is `[SessionSummary]` from FFI (Rust already truncated to 100). We map to `[SidebarSessionItem]`. Each item is small (id, title, timestamps, empty preview). So at most 500 small structs.
- **Lifetime:** App (AppViewModel).
- **Bounded?** **Yes.** Cap 500; items are lightweight.

---

## 10. Swift: `SessionDetailViewModel.messages` and `sessionEvents`

**File:** `SessionModels.swift` (lines 48–49, 62–66, 77–79)

```swift
@Published private(set) var messages: [ChatMessage] = []
@Published private(set) var sessionEvents: [LogEvent] = []
// appendMessage trims to maxMessagesCount (500)
// sessionEvents = Array(events.prefix(maxSessionEventsCount)) (2000)
```

- **How memory grows:** `messages` and `sessionEvents` are capped at 500 and 2000. But `sessionEvents` is filled from `events` which is the **full** array from FFI (see 3). So we still **decode** the full array; only storage is capped. `LogEvent.details` can be large (JSON string).
- **Lifetime:** Session (one SessionDetailViewModel at a time).
- **Bounded?** **Storage** is bounded (2000 events). **Decode** is not (see 3).

---

## 11. Swift: `runAgentRequest` / `AgentResponse.answer`

**File:** `oxcer_ffi/src/lib.rs` (AgentResponse); `ContentView.swift` (runAgentRequest)

- **How memory grows:** `AgentResponse { answer: Option<String> }` can hold a full LLM response (e.g. 10k–100k characters). One large String per request. Then we append one `ChatMessage` with that content into `activeSessionDetail.messages`.
- **Lifetime:** Request (then one copy kept in session messages, capped by 500).
- **Bounded?** **Yes** at session level (message cap). Single response can still be large (tens of KB to MB).

---

## 12. Rust: `LocalPhi3Engine::generate` — prompt and output

**File:** `oxcer-core/src/llm/local_phi3/mod.rs` (lines 53–85)

```rust
let encoding = self.tokenizer.encode(prompt, true)?;
let input_ids: Vec<u32> = encoding.get_ids().to_vec();  // copy
let output_ids = self.runtime.generate(&input_ids, params)?;
let decoded = self.tokenizer.decode(&output_ids, true)?;
Ok(decoded)  // String, can be long
```

- **How memory grows:** Prompt string, `input_ids`, `output_ids`, and decoded `String` all in memory for the duration of the call. Large context -> large allocations.
- **Lifetime:** Request.
- **Bounded?** Only by model/token limits; no explicit cap in this layer.

---

## 13. Rust: `ensure_local_model_impl` — model load and file copy

**File:** `oxcer_ffi/src/lib.rs` (lines 283–291, 224–244)

- **How memory grows:** `LocalPhi3Engine::new(&model_root)` loads the full model (~2.3 GB) into memory. Optionally `std::fs::copy` for `model.gguf` (another 2.3 GB on disk during copy). One-time but very large.
- **Lifetime:** App (GLOBAL_ENGINE).
- **Bounded?** **Yes.** Single load, known size.

---

## 14. Swift: ShimmerModifier / ModelDownloadOverlay — `while !Task.isCancelled` loops

**File:** `ContentView.swift` (ShimmerModifier, 146–152); `ModelDownloadOverlay.swift` (pulse task)

- **How memory grows:** Loops run until cancelled; each iteration does small work (phase/opacity update). Not allocating large buffers, but if not cancelled they run forever. Already tied to view/scenePhase.
- **Lifetime:** View (session).
- **Bounded?** **Yes.** No unbounded allocations; correctness/leak risk only if lifecycle is wrong.

---

## 15. Swift: UniffiHandleMap and continuation map

**File:** `oxcer_ffi.swift` (handleMap for DownloadCallback, uniffiContinuationHandleMap)

- **How memory grows:** Handles inserted for callbacks/continuations; removed when done. If Rust never releases a callback or a future never completes, handle (and referenced object) could stay.
- **Lifetime:** App (static maps).
- **Bounded?** **Yes** in normal use; risk is leaks if FFI contract is violated.

---

## 16. Rust: `list_sessions_from_dir` — per-file `Vec<LogEvent>` and `content`

**File:** `oxcer-core/src/telemetry.rs` (229–241)

```rust
let content = match std::fs::read_to_string(&path) { ... };
let lines: Vec<&str> = content.lines().filter(...).collect();
let mut events = Vec::with_capacity(lines.len());
for line in &lines {
    if let Ok(ev) = serde_json::from_str::<LogEvent>(line) {
        events.push(ev);
    }
}
```

- **How memory grows:** For one session file: `content` = full file; `events` = all parsed LogEvents. No cap on file size or event count. Worst case one file = hundreds of MB.
- **Lifetime:** Request (one list_sessions call).
- **Bounded?** **No** per file.

---

## 17. Swift: `DetailView` / `ChatDetailContent` — LazyVStack over full arrays

**File:** `ContentView.swift` (ChatDetailContent, ForEach over messages and sessionEvents)

- **How memory grows:** Data is already in `SessionDetailViewModel`; views don’t add more. But we do render up to 500 messages + 2000 events; SwiftUI may hold view state for visible + some off-screen. Identity/diffing can retain more if ids are unstable.
- **Lifetime:** Session.
- **Bounded?** **Yes** (arrays are capped). Risk is view state and identity, not raw array growth.

---

## 18. FFI: `FfiConverterSequenceTypeLogEvent.read` — no streaming

**File:** `oxcer_ffi.swift` (sequence read)

- Same as (3): decoding is strictly sequential and builds the full `[LogEvent]` in memory. No way to “read only first N” at the FFI layer with current API.
- **Bounded?** **No.**

---

## 19. Rust: `log_event_to_ffi` — `details.to_string()`

**File:** `oxcer_ffi/src/lib.rs` (406–417)

```rust
details: Some(e.details.to_string()),
```

- **How memory grows:** Each `LogEvent`’s `details` (serde_json::Value) is converted to String. Large JSON -> large string per event. Combined with unbounded event count this multiplies.
- **Lifetime:** Request (during load_session_log).
- **Bounded?** **No** (event count unbounded).

---

## 20. Swift: `workspaces` and `AppViewModel` app-lifetime state

**File:** `ContentView.swift` (AppViewModel)

- **How memory grows:** `workspaces: [WorkspaceInfo]` is typically small (tens of items). Not a growth risk.
- **Lifetime:** App.
- **Bounded?** **Yes.**

---

# Part 2: Structural Improvements (App-Wide)

## 2.1 Target architecture

- **App lifetime:** Settings, theme, backend handle, **lightweight** session list only (id, title, timestamps, optional tiny preview). No full messages, no full logs, no large buffers.
- **Session lifetime:** Full messages and full log for **one** active session only. One `SessionDetailViewModel`; replace on session change / new chat.
- **Request lifetime:** Temporary buffers (prompt, context, single response string, download progress), released when the request ends.

## 2.2 Structural changes

1. **Cap and trim at the source (Rust)**  
   - `load_session_log_from_dir`: do not read unbounded file. Either:
     - read in a streaming/chunked way and stop after N events (e.g. last 2000), or  
     - read tail of file only (e.g. last M bytes), or  
     - enforce a hard cap (e.g. last 2000 lines) before building `Vec<LogEvent>`.  
   - Ensures Rust never returns 500k events; Swift never decodes 500k events.

2. **Cap list_sessions input**  
   - In `list_sessions_from_dir`: when building summaries, do not read full file for huge sessions. Options:
     - Cap per-file read (e.g. first 50 KB or first 100 lines) to derive start/end/costs, or  
     - Skip sessions whose file size exceeds a threshold.  
   - Prevents one 500 MB file from being read into memory during list.

3. **Don’t reload full session list on every agent response**  
   - Remove or throttle `await loadSessions()` after `runAgentRequest`. Reload only on explicit refresh or on a timer with long interval. Reduces repeated full scans and allocations.

4. **Single source of truth for heavy data**  
   - Already done: only `SessionDetailViewModel` holds messages + sessionEvents; one at a time. Keep this; ensure no duplicate copies (e.g. no caching of full log in AppViewModel).

5. **Optional: streaming or paged session log API**  
   - New API: e.g. `load_session_log_tail(session_id, limit)` or paged `load_session_log_page(session_id, offset, limit)`. UI requests only what it needs. Requires API/FFI change; high impact, higher effort.

---

# Part 3: Concrete Refactor Suggestions per Site

---

### Site 1 & 2 & 3: Unbounded session log load (Rust + FFI + Swift decode)

- **Fix (Rust):** In `load_session_log_from_dir`:
  - Read file line-by-line (e.g. `BufReader` + lines()) or in chunks.
  - Keep only the **last** `max_events` (e.g. 2000) events (e.g. ring buffer or skip N - 2000 then collect).
  - Or: read only the last M bytes of the file (e.g. `seek` to end minus 2 MB) then parse lines.
- **Files:** `oxcer-core/src/telemetry.rs` (`load_session_log_from_dir`).
- **Tradeoff:** Slightly more complex I/O; avoids multi-GB allocations.
- **Impact: HIGH.**

- **Fix (Swift):** Keep `Array(events.prefix(maxSessionEventsCount))` but ensure Rust never returns more than a few thousand events (so Swift decode is bounded).
- **Impact: HIGH** (when combined with Rust cap).

---

### Site 4 & 16: list_sessions reads full files

- **Fix (Rust):** In `list_sessions_from_dir`, for each session file:
  - Cap read size: e.g. `std::fs::File::open` + `take(1024 * 100)` (100 KB) or read first N lines only.
  - Or skip files over a size threshold (`metadata().len() > MAX_SESSION_FILE_SIZE`).
  - Build `SessionSummary` from the partial data (start from first line, end from last line in chunk; approximate if needed).
- **Files:** `oxcer-core/src/telemetry.rs` (`list_sessions_from_dir`).
- **Tradeoff:** Summary might be approximate for huge files; avoids loading huge files.
- **Impact: HIGH.**

---

### Site 5: loadSessions() after every agent response

- **Fix (Swift):** Remove `await loadSessions()` from `runAgentRequest` success path. Call `loadSessions()` only:
  - on initial load,
  - on explicit refresh (e.g. refresh button),
  - and optionally on a long-interval timer (e.g. 5 minutes) if you want periodic refresh.
- **Files:** `apps/OxcerLauncher/OxcerLauncher/ContentView.swift` (`runAgentRequest`).
- **Tradeoff:** Session list may be slightly stale until refresh; large reduction in redundant work and allocation spikes.
- **Impact: HIGH.**

---

### Site 6: Download progress callback frequency

- **Fix (Rust):** In `downloader.rs`, throttle progress callbacks (e.g. at most every 250 ms or every 1% progress). Reuse a single message string or format only when needed.
- **Files:** `oxcer-core/src/llm/downloader.rs`.
- **Tradeoff:** Slightly less smooth progress; fewer allocations and less main-queue churn. Swift already throttles; Rust-side throttle reduces cross-FFI traffic.
- **Impact: MEDIUM.**

---

### Site 7 & 8: FFI buffer copies

- **Fix:** No change to UniFFI generated code. Bounding the **size** of data passed (Rust caps in 1–2) keeps buffer sizes bounded. Optional: if a future API supports “load last N events”, the returned buffer is small by construction.
- **Impact: LOW** once Rust caps are in place.

---

### Site 9: sessions array

- **Fix:** Already bounded (500, and Rust returns 100). Optionally reduce `maxSessionsCount` to 100 to match Rust.
- **Files:** `ContentView.swift` (AppViewModel).
- **Impact: LOW.**

---

### Site 10: SessionDetailViewModel.sessionEvents

- **Fix:** Ensure events are capped **before** Swift receives them (Rust cap in 1–2). Then `Array(events.prefix(2000))` only trims if Rust ever sends more; no need to decode 500k.
- **Impact: HIGH** (via Rust cap).

---

### Site 11: AgentResponse.answer size

- **Fix (optional):** If LLM can return very long answers, consider truncating or summarizing before appending to messages (e.g. cap at 50k characters and append “… (truncated)”). Or show “too long” in UI and don’t store full string.
- **Files:** `ContentView.swift` (`runAgentRequest`), or Rust response builder.
- **Tradeoff:** User might not see full answer in UI; avoids single huge message.
- **Impact: MEDIUM** (only if responses are routinely huge).

---

### Site 12: LocalPhi3Engine generate buffers

- **Fix (optional):** Enforce max prompt length / max output tokens in `GenerationParams` or in the caller. Reduces peak allocation for one request.
- **Files:** `oxcer-core/src/llm` (params or generate caller).
- **Impact: MEDIUM.**

---

### Site 13: Model load

- **Fix:** None; single 2.3 GB load is expected. Ensure it runs only once (already gated).
- **Impact: N/A.**

---

### Site 14: Animation loops

- **Fix:** Already tied to view and scenePhase; ensure no extra retain cycles. No change needed unless profiling shows otherwise.
- **Impact: LOW.**

---

### Site 15: Handle maps

- **Fix:** Ensure Rust releases callbacks and completes futures. Add defensive cleanup on Swift side if a session or window is torn down (e.g. cancel in-flight FFI work). No code change required for normal operation.
- **Impact: LOW.**

---

### Site 17: LazyVStack / view state

- **Fix:** Keep `ForEach(..., id: \.self)` for sessionEvents. Ensure `ChatMessage` and `LogEvent` identities are stable. If needed, add explicit `id` (e.g. index or stable id) to avoid unnecessary view churn. No structural change.
- **Impact: LOW.**

---

### Site 19: details.to_string() per event

- **Fix:** Add Rust-side cap on event count (site 1–2). Optionally truncate `details` in Rust to e.g. first 2 KB per event when converting to FFI, if single events can be huge.
- **Files:** `oxcer_ffi/src/lib.rs` (`log_event_to_ffi`), or in telemetry when writing.
- **Impact: MEDIUM** (if details are routinely large).

---

# Part 4: Summary Table

| # | Location | How memory grows | Lifetime | Bounded? | Suggested fix | Impact |
|---|----------|------------------|----------|---------|---------------|--------|
| 1 | telemetry.rs `load_session_log_from_dir` | Full file + full Vec<LogEvent> | Request | No | Cap events (e.g. last 2000), stream/tail read | HIGH |
| 2 | oxcer_ffi load_session_log | Full Vec returned to FFI | Request | No | Cap in Rust (1) | HIGH |
| 3 | oxcer_ffi.swift FfiConverterSequenceTypeLogEvent.read | Full [LogEvent] decoded | Request | No | Cap in Rust so buffer small | HIGH |
| 4 | telemetry.rs list_sessions_from_dir | Full file per session | Request | No | Cap per-file read size or skip huge files | HIGH |
| 5 | ContentView runAgentRequest -> loadSessions() | Reload all sessions every response | Request (repeated) | No | Remove or throttle loadSessions() after response | HIGH |
| 6 | downloader.rs on_progress per chunk | Many format! + callback | Request | Yes | Throttle in Rust | MEDIUM |
| 7–8 | FFI lower/Data(rustBuffer:) | Copies; decode full | Request | Depends | Bound payload size (Rust caps) | LOW/MEDIUM |
| 9 | sessions / SidebarSessionItem | Small list | App | Yes | Optional: match Rust 100 cap | LOW |
| 10 | SessionDetailViewModel | Capped storage; decode unbounded | Session | Decode no | Cap in Rust (1) | HIGH |
| 11 | AgentResponse.answer | One large String per response | Request | Yes (message cap) | Optional: truncate very long answers | MEDIUM |
| 12 | LocalPhi3Engine generate | Prompt + output buffers | Request | By token limit | Optional: explicit max tokens | MEDIUM |
| 13 | ensure_local_model | 2.3 GB model load | App | Yes | None | N/A |
| 14–15 | Loops, handle maps | Correctness / leaks | View/App | Yes | Lifecycle already addressed | LOW |
| 16 | list_sessions per-file (duplicate of 4) | Same as 4 | Request | No | Same as 4 | HIGH |
| 17 | ChatDetailContent | View state | Session | Yes | Identity stable | LOW |
| 18 | Sequence read (duplicate of 3) | Same as 3 | Request | No | Same as 3 | HIGH |
| 19 | log_event_to_ffi details | Large string per event | Request | No | Rust cap + optional details truncation | MEDIUM |

---

# Part 5: Recommended Implementation Order

1. **Rust: Cap `load_session_log_from_dir`** (e.g. last 2000 events or last N bytes). Stops unbounded session log load and bounds FFI buffer and Swift decode. **HIGH.**
2. **Swift: Stop calling `loadSessions()` after every agent response.** Only call on init and on explicit refresh. **HIGH.**
3. **Rust: Cap or limit per-file read in `list_sessions_from_dir`** (e.g. first 100 KB or skip files > size limit). **HIGH.**
4. **Rust: Throttle download progress** in `downloader.rs`. **MEDIUM.**
5. **Optional: Truncate very long LLM answers** before appending to messages. **MEDIUM.**
6. **Optional: Truncate `LogEvent.details`** in Rust when converting to FFI (e.g. first 2 KB). **MEDIUM.**

This order addresses the largest and most likely causes of multi-GB growth (unbounded log load, repeated full session list reload, and unbounded per-file read in list_sessions) first, then refines with throttling and optional truncation.
