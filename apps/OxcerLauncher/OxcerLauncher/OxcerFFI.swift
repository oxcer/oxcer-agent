//  OxcerFFI.swift
//  OxcerLauncher
//
//  Swift wrapper around the Rust oxcer_ffi C API. All FFI functions exchange UTF-8 JSON;
//  this type decodes/encodes via Codable and calls oxcer_string_free on returned pointers.

import Foundation
import Darwin

// MARK: - JSON payloads (mirror Rust contracts)

public struct WorkspaceInfo: Codable {
    public let id: String
    public let name: String
    public let rootPath: String

    enum CodingKeys: String, CodingKey {
        case id, name
        case rootPath = "root_path"
    }

    public init(id: String, name: String, rootPath: String) {
        self.id = id
        self.name = name
        self.rootPath = rootPath
    }
}

public struct SessionSummary: Codable {
    public let sessionId: String
    public let startTimestamp: String
    public let endTimestamp: String
    public let totalCostUsd: Double
    public let success: Bool
    public let toolCallsCount: UInt32
    public let approvalsCount: UInt32
    public let deniesCount: UInt32

    enum CodingKeys: String, CodingKey {
        case sessionId = "session_id"
        case startTimestamp = "start_timestamp"
        case endTimestamp = "end_timestamp"
        case totalCostUsd = "total_cost_usd"
        case success
        case toolCallsCount = "tool_calls_count"
        case approvalsCount = "approvals_count"
        case deniesCount = "denies_count"
    }

    public init(sessionId: String, startTimestamp: String, endTimestamp: String, totalCostUsd: Double, success: Bool, toolCallsCount: UInt32, approvalsCount: UInt32, deniesCount: UInt32) {
        self.sessionId = sessionId
        self.startTimestamp = startTimestamp
        self.endTimestamp = endTimestamp
        self.totalCostUsd = totalCostUsd
        self.success = success
        self.toolCallsCount = toolCallsCount
        self.approvalsCount = approvalsCount
        self.deniesCount = deniesCount
    }
}

public struct LogMetrics: Codable {
    public let tokensIn: UInt32?
    public let tokensOut: UInt32?
    public let latencyMs: UInt64?
    public let costUsd: Double?

    enum CodingKeys: String, CodingKey {
        case tokensIn = "tokens_in"
        case tokensOut = "tokens_out"
        case latencyMs = "latency_ms"
        case costUsd = "cost_usd"
    }

    public init(tokensIn: UInt32? = nil, tokensOut: UInt32? = nil, latencyMs: UInt64? = nil, costUsd: Double? = nil) {
        self.tokensIn = tokensIn
        self.tokensOut = tokensOut
        self.latencyMs = latencyMs
        self.costUsd = costUsd
    }
}

public struct LogEvent: Codable {
    public let timestamp: String
    public let sessionId: String
    public let requestId: String?
    public let caller: String
    public let component: String
    public let action: String
    public let decision: String?
    public let metrics: LogMetrics
    /// Arbitrary JSON (object, array, or primitive) from Rust.
    public let details: AnyCodableValue?

    enum CodingKeys: String, CodingKey {
        case timestamp
        case sessionId = "session_id"
        case requestId = "request_id"
        case caller, component, action, decision, metrics, details
    }

    public init(timestamp: String, sessionId: String, requestId: String?, caller: String, component: String, action: String, decision: String?, metrics: LogMetrics, details: AnyCodableValue?) {
        self.timestamp = timestamp
        self.sessionId = sessionId
        self.requestId = requestId
        self.caller = caller
        self.component = component
        self.action = action
        self.decision = decision
        self.metrics = metrics
        self.details = details
    }
}

/// Type-erased JSON value for LogEvent.details (Rust sends arbitrary JSON).
public enum AnyCodableValue: Codable {
    case null
    case string(String)
    case int(Int)
    case double(Double)
    case bool(Bool)
    case object([String: AnyCodableValue])
    case array([AnyCodableValue])

    public init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null; return }
        if let s = try? c.decode(String.self) { self = .string(s) }
        else if let i = try? c.decode(Int.self) { self = .int(i) }
        else if let d = try? c.decode(Double.self) { self = .double(d) }
        else if let b = try? c.decode(Bool.self) { self = .bool(b) }
        else if let o = try? c.decode([String: AnyCodableValue].self) { self = .object(o) }
        else if let a = try? c.decode([AnyCodableValue].self) { self = .array(a) }
        else { throw DecodingError.dataCorruptedError(in: c, debugDescription: "AnyCodableValue") }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .null: try c.encodeNil()
        case .string(let s): try c.encode(s)
        case .int(let i): try c.encode(i)
        case .double(let d): try c.encode(d)
        case .bool(let b): try c.encode(b)
        case .object(let o): try c.encode(o)
        case .array(let a): try c.encode(a)
        }
    }

    /// Pretty-printed JSON for display in UI (e.g. event details).
    public var jsonString: String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        guard let data = try? encoder.encode(self) else { return "{}" }
        return String(data: data, encoding: .utf8) ?? "{}"
    }
}

public struct TaskContext: Codable {
    public var workspaceId: String?
    public var selectedPaths: [String]?
    public var riskHints: Bool?

    enum CodingKeys: String, CodingKey {
        case workspaceId = "workspace_id"
        case selectedPaths = "selected_paths"
        case riskHints = "risk_hints"
    }

    public init(workspaceId: String? = nil, selectedPaths: [String]? = nil, riskHints: Bool? = nil) {
        self.workspaceId = workspaceId
        self.selectedPaths = selectedPaths
        self.riskHints = riskHints
    }
}

public struct AgentRequestPayload: Codable {
    public var taskDescription: String
    public var workspaceId: String?
    public var workspaceRoot: String?
    public var context: TaskContext?
    public var appConfigDir: String?

    enum CodingKeys: String, CodingKey {
        case taskDescription = "task_description"
        case workspaceId = "workspace_id"
        case workspaceRoot = "workspace_root"
        case context
        case appConfigDir = "app_config_dir"
    }

    public init(taskDescription: String, workspaceId: String? = nil, workspaceRoot: String? = nil, context: TaskContext? = nil, appConfigDir: String? = nil) {
        self.taskDescription = taskDescription
        self.workspaceId = workspaceId
        self.workspaceRoot = workspaceRoot
        self.context = context
        self.appConfigDir = appConfigDir
    }
}

public struct AgentResponse: Codable {
    public let ok: Bool
    public let answer: String?
    public let error: String?

    public init(ok: Bool, answer: String? = nil, error: String? = nil) {
        self.ok = ok
        self.answer = answer
        self.error = error
    }
}

public struct WorkspacesResponse: Codable {
    public let workspaces: [WorkspaceInfo]
}

// MARK: - FFI errors

public enum OxcerFFIError: LocalizedError {
    case libraryNotLoaded(String)
    case invalidUTF8
    case invalidJSON(String)
    case rustError(String)

    public var errorDescription: String? {
        switch self {
        case .libraryNotLoaded(let msg): return "Oxcer library not loaded: \(msg)"
        case .invalidUTF8: return "Invalid UTF-8 from Rust"
        case .invalidJSON(let msg): return "Invalid JSON: \(msg)"
        case .rustError(let msg): return msg
        }
    }
}

// MARK: - OxcerFFI (C calls via bridging header)
//
// The Rust dylib (liboxcer_ffi.dylib) is embedded in the app bundle at Contents/PlugIns and
// loaded at runtime via the runpath @executable_path/../PlugIns (no absolute paths).

public final class OxcerFFI {

    /// Call Rust: list workspaces from config. Pass nil for appConfigDir to use default (e.g. ~/Library/Application Support/Oxcer).
    public static func listWorkspaces(appConfigDir: String? = nil) throws -> [WorkspaceInfo] {
        let input: [String: Any] = appConfigDir.map { ["app_config_dir": $0] } ?? [:]
        let jsonStr = try jsonString(from: input)
        return try jsonStr.withCString { cStr in
            let out = try callAndFree { oxcer_list_workspaces(cStr) }
            let decoded = try JSONDecoder().decode(WorkspacesResponse.self, from: out)
            return decoded.workspaces
        }
    }

    /// Call Rust: list recent sessions. Pass nil for appConfigDir to use default.
    public static func listSessions(appConfigDir: String? = nil) throws -> [SessionSummary] {
        let input: [String: Any] = appConfigDir.map { ["app_config_dir": $0] } ?? [:]
        let jsonStr = try jsonString(from: input)
        return try jsonStr.withCString { cStr in
            let out = try callAndFree { oxcer_list_sessions(cStr) }
            if let err = try? JSONDecoder().decode(ErrorResponse.self, from: out) {
                throw OxcerFFIError.rustError(err.error)
            }
            return try JSONDecoder().decode([SessionSummary].self, from: out)
        }
    }

    /// Call Rust: load session log events for one session.
    public static func loadSessionLog(sessionId: String, appConfigDir: String? = nil) throws -> [LogEvent] {
        var input: [String: Any] = ["session_id": sessionId]
        if let dir = appConfigDir { input["app_config_dir"] = dir }
        let jsonStr = try jsonString(from: input)
        return try jsonStr.withCString { cStr in
            let out = try callAndFree { oxcer_load_session_log(cStr) }
            if let err = try? JSONDecoder().decode(ErrorResponse.self, from: out) {
                throw OxcerFFIError.rustError(err.error)
            }
            return try JSONDecoder().decode([LogEvent].self, from: out)
        }
    }

    /// Call Rust: run agent request (task). With the current stub executor, this will error when the plan requires tools; use step API from the app for full execution.
    public static func agentRequest(_ payload: AgentRequestPayload) throws -> AgentResponse {
        let input = payloadToDict(payload)
        let jsonStr = try jsonString(from: input)
        return try jsonStr.withCString { cStr in
            let out = try callAndFree { oxcer_agent_request(cStr) }
            let decoded = try JSONDecoder().decode(AgentResponse.self, from: out)
            if !decoded.ok, let err = decoded.error {
                throw OxcerFFIError.rustError(err)
            }
            return decoded
        }
    }

    // MARK: - Helpers

    private struct ErrorResponse: Codable {
        let error: String
    }

    private static func jsonString(from obj: [String: Any]) throws -> String {
        let data = try JSONSerialization.data(withJSONObject: obj)
        guard let s = String(data: data, encoding: .utf8) else { throw OxcerFFIError.invalidUTF8 }
        return s
    }

    private static func payloadToDict(_ p: AgentRequestPayload) -> [String: Any] {
        var d: [String: Any] = ["task_description": p.taskDescription]
        if let v = p.workspaceId { d["workspace_id"] = v }
        if let v = p.workspaceRoot { d["workspace_root"] = v }
        if let v = p.appConfigDir { d["app_config_dir"] = v }
        if let ctx = p.context {
            var ctxD: [String: Any] = [:]
            if let v = ctx.workspaceId { ctxD["workspace_id"] = v }
            if let v = ctx.selectedPaths { ctxD["selected_paths"] = v }
            if let v = ctx.riskHints { ctxD["risk_hints"] = v }
            if !ctxD.isEmpty { d["context"] = ctxD }
        }
        return d
    }

    private static func callAndFree(_ body: () -> UnsafePointer<CChar>?) throws -> Data {
        guard let ptr = body() else {
            throw OxcerFFIError.rustError("Rust returned null")
        }
        defer { oxcer_string_free(UnsafeMutablePointer(mutating: ptr)) }
        let len = strlen(ptr)
        let buffer = UnsafeBufferPointer(start: ptr, count: len)
        guard let str = String(bytes: buffer.map { UInt8($0) }, encoding: .utf8) else {
            throw OxcerFFIError.invalidUTF8
        }
        guard let data = str.data(using: .utf8) else {
            throw OxcerFFIError.invalidUTF8
        }
        return data
    }
}
