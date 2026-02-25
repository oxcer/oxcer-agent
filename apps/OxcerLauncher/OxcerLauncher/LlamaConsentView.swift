//  LlamaConsentView.swift
//  OxcerLauncher
//
//  First-run consent screen for Meta Llama 3.
//  Shown once before the model is loaded or downloaded.
//  Acceptance is persisted via @AppStorage("hasAcceptedLlamaLicense") (UserDefaults).
//
//  Compliance rationale:
//    Meta Llama 3 Community License §1.b requires prominently displaying
//    "Built with Meta Llama 3" and §1.b.iv requires users to comply with
//    the Acceptable Use Policy. This view satisfies both requirements for
//    the in-app surface.

import SwiftUI

// MARK: - LlamaConsentView

/// Shown on first launch (or whenever hasAcceptedLlamaLicense == false).
/// The user must actively tap "Accept & Continue" before the model is loaded
/// or downloaded. Tapping "Decline" terminates the app.
struct LlamaConsentView: View {
    /// Persisted acceptance flag. Set to true only by the "Accept & Continue" button.
    @AppStorage("hasAcceptedLlamaLicense") private var hasAcceptedLlamaLicense: Bool = false

    /// Called after the user accepts. Use to trigger model load / download.
    let onAccept: () -> Void
    /// Called when the user declines. Typically: NSApp.terminate(nil).
    let onDecline: () -> Void

    var body: some View {
        VStack(spacing: 0) {

            // ── Header ──────────────────────────────────────────────────────
            VStack(spacing: 14) {
                Image(systemName: "brain.head.profile")
                    .font(.system(size: 44))
                    .foregroundStyle(OxcerTheme.accent)

                Text("Built with Meta Llama 3")
                    .font(.title2.weight(.semibold))
                    .foregroundStyle(OxcerTheme.textPrimary)

                Text(
                    "Oxcer uses Meta Llama 3 8B Instruct for on-device AI inference. " +
                    "Before continuing, please review the model license and usage policy."
                )
                .font(.subheadline)
                .foregroundStyle(OxcerTheme.textSecondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.top, 36)
            .padding(.horizontal, 32)

            // ── License links ────────────────────────────────────────────────
            VStack(alignment: .leading, spacing: 10) {
                ConsentLinkRow(
                    icon: "doc.text.fill",
                    title: "Meta Llama 3 Community License",
                    subtitle: "Governs use of the Llama 3 model weights.",
                    url: URL(string: "https://llama.meta.com/llama3/license/")!
                )
                ConsentLinkRow(
                    icon: "hand.raised.fill",
                    title: "Meta Llama 3 Acceptable Use Policy",
                    subtitle: "Describes permitted and prohibited uses of the model.",
                    url: URL(string: "https://llama.meta.com/llama3/use-policy/")!
                )
            }
            .padding(.horizontal, 24)
            .padding(.top, 24)

            // ── Consent bullet points ────────────────────────────────────────
            VStack(alignment: .leading, spacing: 8) {
                Text("By tapping \"Accept & Continue\" you confirm that:")
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(OxcerTheme.textPrimary)

                ConsentBullet("You have read and agree to the Meta Llama 3 Community License.")
                ConsentBullet("Your use of the model complies with the Meta Llama 3 Acceptable Use Policy.")
                ConsentBullet(
                    "You understand that Oxcer (MIT License) and Meta Llama 3 " +
                    "(Meta Llama 3 Community License) are separately licensed components."
                )
            }
            .padding(16)
            .background(
                RoundedRectangle(cornerRadius: 12)
                    .fill(Color("OxcerSurface"))
            )
            .padding(.horizontal, 24)
            .padding(.top, 16)

            Spacer(minLength: 16)

            // ── Action buttons ───────────────────────────────────────────────
            HStack(spacing: 12) {
                Button("Decline") {
                    onDecline()
                }
                .buttonStyle(.bordered)
                .foregroundStyle(OxcerTheme.textSecondary)
                .keyboardShortcut(.escape, modifiers: [])

                Button("Accept & Continue") {
                    hasAcceptedLlamaLicense = true
                    onAccept()
                }
                .buttonStyle(.borderedProminent)
                .tint(OxcerTheme.accent)
                .keyboardShortcut(.return, modifiers: [])
            }
            .padding(.horizontal, 24)
            .padding(.bottom, 16)

            // ── Attribution footer (verbatim required text) ─────────────────
            Text(
                "Meta Llama 3 is licensed under the Meta Llama 3 Community License, " +
                "Copyright © Meta Platforms, Inc. All Rights Reserved."
            )
            .font(.caption2)
            .foregroundStyle(OxcerTheme.textTertiary)
            .multilineTextAlignment(.center)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.horizontal, 32)
            .padding(.bottom, 24)
        }
        .frame(width: 520, height: 600)
        .background(Color("OxcerBackground"))
    }
}

// MARK: - Subviews

private struct ConsentLinkRow: View {
    let icon: String
    let title: String
    let subtitle: String
    let url: URL

    @Environment(\.openURL) private var openURL

    var body: some View {
        Button { openURL(url) } label: {
            HStack(spacing: 12) {
                Image(systemName: icon)
                    .font(.system(size: 18))
                    .foregroundStyle(OxcerTheme.accent)
                    .frame(width: 28)

                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.subheadline.weight(.medium))
                        .foregroundStyle(OxcerTheme.textPrimary)
                    Text(subtitle)
                        .font(.caption)
                        .foregroundStyle(OxcerTheme.textSecondary)
                }

                Spacer(minLength: 0)

                Image(systemName: "arrow.up.right.square")
                    .font(.caption)
                    .foregroundStyle(OxcerTheme.textTertiary)
            }
            .padding(12)
            .background(
                RoundedRectangle(cornerRadius: 8)
                    .fill(OxcerTheme.accent.opacity(0.07))
            )
        }
        .buttonStyle(.plain)
    }
}

private struct ConsentBullet: View {
    let text: String
    init(_ text: String) { self.text = text }

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Text("•")
                .font(.caption)
                .foregroundStyle(OxcerTheme.accent)
                .padding(.top, 1)
            Text(text)
                .font(.caption)
                .foregroundStyle(OxcerTheme.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
