//  KeychainHelper.swift
//  OxcerLauncher
//
//  Secure storage for sensitive data (e.g., API keys) using macOS Keychain.
//  @KeychainStorage mimics @AppStorage but persists to Keychain instead of UserDefaults.

import Foundation
import Security
import SwiftUI

// MARK: - KeychainHelper

enum KeychainHelper {
    private static let serviceName = Bundle.main.bundleIdentifier ?? "com.oxcer.launcher"

    /// Loads a string from Keychain for the given key. Returns nil if not found.
    static func load(key: String) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: serviceName,
            kSecAttrAccount as String: key,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]

        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)

        guard status == errSecSuccess,
              let data = result as? Data,
              let string = String(data: data, encoding: .utf8)
        else {
            return nil
        }
        return string
    }

    /// Saves a string to Keychain. Pass empty string to delete.
    static func save(key: String, value: String) {
        if value.isEmpty {
            delete(key: key)
            return
        }

        guard let data = value.data(using: .utf8) else { return }

        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: serviceName,
            kSecAttrAccount as String: key,
        ]

        let attributes: [String: Any] = [
            kSecValueData as String: data,
        ]

        // Try to update first; if not found, add
        let status = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)

        if status == errSecItemNotFound {
            let addQuery = query.merging(attributes) { _, new in new }
            SecItemAdd(addQuery as CFDictionary, nil)
        }
    }

    /// Deletes the item for the given key.
    static func delete(key: String) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: serviceName,
            kSecAttrAccount as String: key,
        ]
        SecItemDelete(query as CFDictionary)
    }
}

// MARK: - KeychainStorage Property Wrapper

/// A property wrapper that stores a String securely in the Keychain.
/// Mimics `@AppStorage` behavior but uses Keychain instead of UserDefaults.
@propertyWrapper
struct KeychainStorage: DynamicProperty {
    @State private var value: String
    private let key: String

    init(wrappedValue: String, key: String) {
        self.key = key
        let initial = KeychainHelper.load(key: key) ?? wrappedValue
        _value = State(initialValue: initial)
    }

    var wrappedValue: String {
        get { value }
        nonmutating set {
            value = newValue
            KeychainHelper.save(key: key, value: newValue)
        }
    }

    var projectedValue: Binding<String> {
        Binding(
            get: { value },
            set: { newValue in
                value = newValue
                KeychainHelper.save(key: key, value: newValue)
            }
        )
    }
}
