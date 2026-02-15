# Memory Growth Analysis — OxcerLauncher

## Scope

Searched for: infinite loops, timers, `Task.sleep`, periodic work, and ever-growing log/debug buffers.

**OxcerLauncher** uses only: `apps/OxcerLauncher/`, `oxcer_ffi/`, `oxcer-core/` (via FFI).  
The `reference/` folder is a separate project and is not part of OxcerLauncher.

---

## 1. Suspicious sites in OxcerLauncher

### 1.1 ShimmerModifier — `while !Task.isCancelled` loop

**File:** `apps/OxcerLauncher/OxcerLauncher/ContentView.swift` (lines 124–139)

```swift
.onAppear {
    animationTask = Task { @MainActor in
        while !Task.isCancelled {
            withAnimation(.linear(duration: 1.1)) { phase = 1.4 }
            try? await Task.sleep(nanoseconds: 1_100_000_000)
            ...
        }
    }
}
```

| Aspect | Details |
|--------|---------|
| **Lifetime** | Per shimmer view; 5 instances when `isSessionsLoading == true` |
| **Runs when idle?** | Only while sessions are loading; stops when loading finishes |
| **Appends to collection?** | No |
| **Risk** | Low when sessions load quickly. If loading never completes or is retriggered often, 5 loops run until `onDisappear` cancels them. |

---

### 1.2 ModelDownloadOverlay — `while !Task.isCancelled` loop

**File:** `apps/OxcerLauncher/OxcerLauncher/ModelDownloadOverlay.swift` (lines 73–83)

```swift
.onAppear {
    pulseTask = Task { @MainActor in
        while !Task.isCancelled {
            isPulsing = true
            try? await Task.sleep(nanoseconds: 600_000_000)
            ...
        }
    }
}
```

| Aspect | Details |
|--------|---------|
| **Lifetime** | Per overlay; one instance while `!isModelReady` |
| **Runs when idle?** | Only during model setup; stops when overlay is removed |
| **Appends to collection?** | No |
| **Risk** | Low after first run; stops when model is ready |

---

### 1.3 Download progress callback — frequent updates during download

**File:** `oxcer-core/src/llm/downloader.rs` (lines 61–86)

```rust
while let Some(chunk) = stream.next().await {
    ...
    callback_clone.on_progress(progress, format!("Downloading... {}%", pct));
}
```

**Swift side:** `ContentView.swift` — `DispatchQueue.main.async` per callback

| Aspect | Details |
|--------|---------|
| **Lifetime** | Active only during model download |
| **Runs when idle?** | No; only during download |
| **Frequency** | One callback per chunk (~40,000+ for 2.4GB) |
| **Risk** | High during download: floods main queue and `@Published` updates, causing many view redraws and memory churn |

---

### 1.4 UniFFI poll loop — `repeat { } while`

**File:** `apps/OxcerLauncher/OxcerLauncher/oxcer_ffi.swift` (lines 1602–1611)

```swift
repeat {
    pollResult = await withUnsafeContinuation { ... }
} while pollResult != UNIFFI_RUST_FUTURE_POLL_READY
```

| Aspect | Details |
|--------|---------|
| **Lifetime** | Per async FFI call |
| **Runs when idle?** | No; cooperatively suspends |
| **Risk** | None; suspends until Rust resumes |

---

### 1.5 `print()` debug statements

**Files:** `ContentView.swift`, `oxcer_ffi.swift`, `oxcer_ffi/src/lib.rs`

| Aspect | Details |
|--------|---------|
| **Lifetime** | App-wide |
| **Appends to buffer?** | `print()` writes to stdout; no unbounded in-process buffer |
| **Risk** | None for memory growth |

---

### 1.6 UniffiHandleMap

**File:** `oxcer_ffi.swift` — `handleMap`, `uniffiContinuationHandleMap`

| Aspect | Details |
|--------|---------|
| **Lifetime** | App-wide |
| **Growth** | Insert on callback/create; remove on completion. No trimming, but entries are removed. |
| **Risk** | Low if handles are always removed when done |

---

## 2. Rust side (oxcer-core, oxcer_ffi)

- `orchestrator.rs` `loop { }`: used in `agent_request`; runs per request until completion, not a background loop.
- `shell.rs` loops: only in shell execution paths; not used by OxcerLauncher FFI.
- No timers, `tokio::interval`, or polling in the FFI paths used by OxcerLauncher.

---

## 3. Proposed fixes

### Fix 1: Throttle download progress callbacks (high impact during download)

**Problem:** ~40,000 callbacks during a 2.4GB download cause heavy main-queue and view update load.

**Approach:** Throttle progress updates (e.g. ≤ 4 per second) in the Rust downloader or in the Swift callback.

### Fix 2: Tie ShimmerModifier and ModelDownloadOverlay to view lifetime

**Problem:** If the hosting view is recreated often (e.g. by parent identity changes), tasks might not be cancelled quickly.

**Approach:** Use `@Environment(\.scenePhase)` or a similar signal so we stop animations when the window is not active. Optional improvement; `onDisappear` already cancels when the view is removed.

### Fix 3: Optional — disable shimmer when window is not key

**Approach:** Pass an `isActive: Bool` into the shimmer and only run the animation when `isActive` is true, to avoid unnecessary work when the window is in the background.

---

## 4. Summary

| Site | Lifetime | Idle impact? | Ever-growing? | Priority |
|------|----------|--------------|---------------|----------|
| ShimmerModifier loop | Per view | No | No | Low |
| ModelDownloadOverlay loop | Per overlay | No | No | Low |
| Download progress callbacks | During download | No | No | **High (during download)** |
| UniFFI poll loop | Per call | No | No | None |
| print() | App-wide | No | No | None |

For the “idle” case (no model download, no session loading), the main SwiftUI loops should not be active. If memory still grows while idle, possible causes include:

1. SwiftUI internal behavior (e.g. `List`/`LazyVStack` recycling or view identity)
2. A different build or configuration using additional code
3. macOS system frameworks or toolchain behavior

The highest-impact change is throttling download progress callbacks to reduce load during model download.
