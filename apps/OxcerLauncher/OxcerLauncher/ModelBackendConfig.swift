//  ModelBackendConfig.swift
//  OxcerLauncher
//
//  Per-backend generation parameters and a factory that reads UserDefaults
//  to return the correct config for the currently active engine.
//
//  Usage:
//    let config = ModelBackendConfig.current()
//    runner.maxSteps = config.maxSteps
//    runner.config = config

import Foundation

// MARK: - ModelBackendConfig

struct ModelBackendConfig {
    /// Maximum time (seconds) to wait for a single `generateText` call before throwing
    /// `LLMTimeoutError.exceeded`. Prevents the UI from hanging indefinitely on a
    /// slow local model or a stalled cloud request.
    ///
    /// Local Llama: generous (the model is slow for large contexts).
    /// Cloud APIs: 60 s is enough for even very long responses.
    var llmTimeoutSeconds: TimeInterval

    /// Maximum number of `ffi_agent_step` iterations per request.
    /// Higher values allow more complex multi-tool tasks (e.g. enumerate many folders).
    var maxSteps: Int

    var temperature: Float
    var maxTokens: Int

    /// Optional extra text appended to the system prompt for this backend.
    /// Leave empty for cloud providers (they are well-behaved multi-step models).
    /// For local Llama, a continuity hint reduces premature termination.
    var systemPromptSuffix: String

    // MARK: - Named presets

    /// On-device Meta Llama 3 8B via llama.cpp + Metal.
    /// Generous timeout — the model is significantly slower than cloud APIs.
    /// systemPromptSuffix nudges the model to continue through all requested tools
    /// rather than stopping after the first tool result.
    static let localLlama = ModelBackendConfig(
        llmTimeoutSeconds: 120,
        maxSteps: 20,
        temperature: 0.7,
        maxTokens: 2048,
        systemPromptSuffix: "\n\nContinue working through all requested tasks before giving your final answer."
    )

    /// Anthropic Claude (claude-3-5-haiku by default).
    static let anthropic = ModelBackendConfig(
        llmTimeoutSeconds: 60,
        maxSteps: 30,
        temperature: 0.7,
        maxTokens: 4096,
        systemPromptSuffix: ""
    )

    /// OpenAI (gpt-4o-mini by default).
    static let openAI = ModelBackendConfig(
        llmTimeoutSeconds: 60,
        maxSteps: 30,
        temperature: 0.7,
        maxTokens: 4096,
        systemPromptSuffix: ""
    )

    /// Google Gemini (gemini-2.0-flash by default).
    static let gemini = ModelBackendConfig(
        llmTimeoutSeconds: 60,
        maxSteps: 30,
        temperature: 0.7,
        maxTokens: 4096,
        systemPromptSuffix: ""
    )

    /// xAI Grok (grok-2-1212 by default).
    static let grok = ModelBackendConfig(
        llmTimeoutSeconds: 60,
        maxSteps: 30,
        temperature: 0.7,
        maxTokens: 4096,
        systemPromptSuffix: ""
    )

    // MARK: - Active config factory

    /// Returns the config matching the engine currently selected in Settings.
    ///
    /// Reads `useCloudModel` (Bool) and `selectedProvider` (String) from UserDefaults —
    /// the same keys written by `CloudSettingsViewModel`. Returns `.localLlama` when
    /// cloud mode is off or no provider is saved.
    static func current() -> ModelBackendConfig {
        let useCloud = UserDefaults.standard.bool(forKey: "useCloudModel")
        guard useCloud else { return .localLlama }
        let raw = UserDefaults.standard.string(forKey: "selectedProvider") ?? ""
        switch raw {
        case "anthropic": return .anthropic
        case "gemini":    return .gemini
        case "grok":      return .grok
        default:          return .openAI  // "openAI" + any unknown value
        }
    }
}
