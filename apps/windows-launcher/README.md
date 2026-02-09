# Windows Launcher (planned)

This folder is the future home of the **Windows-native launcher** for Oxcer, built with **WinUI 3** and the **Windows App SDK** (C#). It is not implemented yet.

The app will integrate with **oxcer-core** (and possibly **oxcer_ffi**) via FFI or a dedicated CLI interface, consistent with the macOS SwiftUI launcher.

## Planned architecture

```mermaid
flowchart LR
  win[WinUI 3 app]
  oc[oxcer-core]
  win -.->|planned: FFI or CLI| oc
```

*Planned / not implemented.*

## Status

- This is a **stub folder** only; no WinUI project has been created yet.
- It is **not included** in any build, packaging, or release process.
- It will be wired into the toolchain when implementation starts.
