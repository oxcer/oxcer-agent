//  AssistantBlocks.swift
//  OxcerLauncher
//
//  Markdown-block parsing and per-block rendering for assistant messages.
//  Splits assistant text into paragraph runs and fenced code blocks so that
//  code/command output can be rendered with a dedicated Copy button — matching
//  the UX of Claude, ChatGPT, and Gemini.
//
//  Usage:
//    // In AssistantBubble (or any view that has an assistant text string):
//    let blocks = parseMarkdownBlocks(text)
//    ForEach(blocks) { block in … }

import AppKit
import SwiftUI

// MARK: - AssistantBlock

/// One visual segment of an assistant message.
/// Paragraphs are plain text; code blocks are monospaced with a language badge and Copy button.
enum AssistantBlock: Equatable, Identifiable {
    case paragraph(String)
    case code(language: String?, source: String)

    var id: String {
        switch self {
        case .paragraph(let t): return "p-\(t.hashValue)"
        case .code(_, let s):   return "c-\(s.hashValue)"
        }
    }
}

// MARK: - Parser

/// Splits `text` on GitHub-flavoured Markdown fenced code blocks (``` lang\n…\n```).
/// Everything outside a fence is emitted as `.paragraph` after trimming leading/trailing newlines.
/// Returns `[.paragraph(text)]` if no fences are found (zero-cost fast path for plain responses).
func parseMarkdownBlocks(_ text: String) -> [AssistantBlock] {
    // Fast path: skip regex overhead for plain text responses.
    guard text.contains("```") else { return [.paragraph(text)] }

    var blocks: [AssistantBlock] = []
    let pattern = #"```([a-zA-Z0-9+\-]*)\n([\s\S]*?)```"#
    guard let regex = try? NSRegularExpression(pattern: pattern) else {
        return [.paragraph(text)]
    }
    let nsText = text as NSString
    let fullRange = NSRange(location: 0, length: nsText.length)
    var cursor = 0

    for match in regex.matches(in: text, range: fullRange) {
        let matchRange = match.range

        // Text before this code block → paragraph.
        if matchRange.location > cursor {
            let preRange = NSRange(location: cursor, length: matchRange.location - cursor)
            let pre = nsText.substring(with: preRange)
                .trimmingCharacters(in: .whitespacesAndNewlines)
            if !pre.isEmpty {
                blocks.append(.paragraph(pre))
            }
        }

        // Language tag (capture group 1).
        let langRange = match.range(at: 1)
        let lang: String? = langRange.location != NSNotFound
            ? { let s = nsText.substring(with: langRange); return s.isEmpty ? nil : s }()
            : nil

        // Source body (capture group 2).
        let srcRange = match.range(at: 2)
        let src = srcRange.location != NSNotFound
            ? nsText.substring(with: srcRange)
            : ""

        blocks.append(.code(language: lang, source: src))
        cursor = matchRange.location + matchRange.length
    }

    // Remaining text after the last match.
    if cursor < nsText.length {
        let tail = nsText.substring(from: cursor)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if !tail.isEmpty {
            blocks.append(.paragraph(tail))
        }
    }

    return blocks.isEmpty ? [.paragraph(text)] : blocks
}

// MARK: - CodeBlockView

/// Renders a single fenced code block:
///   - Sticky header row: language badge (left) + Copy button (right)
///   - Horizontally scrollable monospaced source body
struct CodeBlockView: View {
    let language: String?
    let source: String

    @State private var showCopied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            headerRow
            Divider().background(OxcerTheme.border)
            sourceBody
        }
        .background(
            RoundedRectangle(cornerRadius: OxcerTheme.cardCornerRadius)
                .fill(OxcerTheme.backgroundPanel)
                .overlay(
                    RoundedRectangle(cornerRadius: OxcerTheme.cardCornerRadius)
                        .stroke(OxcerTheme.border, lineWidth: 1)
                )
        )
        .clipShape(RoundedRectangle(cornerRadius: OxcerTheme.cardCornerRadius))
    }

    private var headerRow: some View {
        HStack {
            if let lang = language, !lang.isEmpty {
                Text(lang.lowercased())
                    .font(.system(.caption2, design: .monospaced, weight: .medium))
                    .foregroundStyle(OxcerTheme.textTertiary)
            }
            Spacer()
            Button { copySource() } label: {
                HStack(spacing: 4) {
                    Image(systemName: showCopied ? "checkmark" : "doc.on.doc")
                        .font(.system(.caption))
                    Text(showCopied ? "Copied" : "Copy")
                        .font(.system(.caption2, weight: .medium))
                }
                .foregroundStyle(
                    showCopied ? OxcerTheme.statusCompleted : OxcerTheme.textTertiary
                )
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(
                    RoundedRectangle(cornerRadius: 6)
                        .fill(OxcerTheme.cardBackground)
                        .overlay(
                            RoundedRectangle(cornerRadius: 6)
                                .stroke(OxcerTheme.border, lineWidth: 1)
                        )
                )
            }
            .buttonStyle(BouncyButtonStyle(scale: 0.93, dimmingOpacity: 0.15))
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(OxcerTheme.cardBackground.opacity(0.5))
    }

    private var sourceBody: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            Text(source)
                .font(.system(.body, design: .monospaced))
                .foregroundStyle(OxcerTheme.textPrimary)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(12)
        }
    }

    private func copySource() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(source, forType: .string)
        withAnimation(OxcerTheme.snappy) { showCopied = true }
        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            withAnimation(OxcerTheme.snappy) { showCopied = false }
        }
    }
}
