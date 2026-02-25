//  ChatInputBar.swift
//  OxcerLauncher
//
//  Isolated chat input: @State localText, zero coupling to AppViewModel during typing.
//  Typing triggers NO parent re-renders.

import SwiftUI

/// Transactional chat input bar. Manages its own typing state; commits on Send only.
/// While generation is running the send button becomes a Stop button.
struct ChatInputBar: View {
    let isRunning: Bool
    let onSend: (String) -> Void
    /// Called when the user taps the Stop button during generation.
    let onStop: () -> Void

    @State private var localText: String = ""
    @FocusState private var isFocused: Bool

    var body: some View {
        HStack(alignment: .bottom, spacing: 12) {
            TextField("Message Oxcer…", text: $localText, axis: .vertical)
                .textFieldStyle(.plain)
                .font(.system(.body))
                .foregroundStyle(OxcerTheme.textPrimary)
                .lineLimit(1 ... 5)
                .focused($isFocused)
                .disabled(isRunning)
                .onKeyPress { press in
                    // Filter: Only handle Return key
                    guard press.key == .return else { return .ignored }

                    // Case 1: Shift + Enter -> Insert Newline Manually
                    if press.modifiers.contains(.shift) {
                        localText.append("\n")
                        return .handled // Stop system from doing weird things
                    }

                    // Case 2: Enter Only -> Send Message
                    if !localText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        submitIfValid()
                        return .handled
                    }

                    return .ignored
                }

            // Send / Stop button.
            // While running: tapping stops the in-flight generation.
            // While idle: tapping sends the composed message.
            Button {
                if isRunning {
                    onStop()
                } else {
                    submitIfValid()
                }
            } label: {
                if isRunning {
                    Image(systemName: "stop.circle.fill")
                        .font(.system(size: 28))
                } else {
                    Image(systemName: "arrow.up.circle.fill")
                        .font(.system(size: 28))
                }
            }
            .buttonStyle(BouncyButtonStyle())
            .foregroundStyle(OxcerTheme.onAccent)
            // Stop button is always enabled; send button requires non-empty text.
            .disabled(!isRunning && localText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            .keyboardShortcut(.return, modifiers: .command)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .background(
            RoundedRectangle(cornerRadius: 12)
                .fill(Color("OxcerSurface"))
                .overlay(
                    RoundedRectangle(cornerRadius: 12)
                        .stroke(
                            isFocused ? OxcerTheme.accent.opacity(0.5) : OxcerTheme.border,
                            lineWidth: isFocused ? 2 : 1
                        )
                )
        )
        // Reclaim keyboard focus when the input bar (re)appears — e.g. after approval bubble dismisses.
        .onAppear { isFocused = true }
    }

    private func submitIfValid() {
        let text = localText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !isRunning else { return }
        localText = ""
        onSend(text)
    }
}
