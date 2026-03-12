//! Environment variable filtering for child processes and display.
//!
//! Before starting any child process we build a filtered environment: drop or mask
//! all keys matching high-risk patterns (AWS_*, GITHUB_*, OPENAI_API_KEY, etc.).
//! When reading env vars to show to the user (or to the agent), use `env_for_display`
//! so the result is run through the data_sensitivity classifier.

use std::collections::HashMap;

use crate::data_sensitivity;

/// High-risk env key patterns (prefix or exact). Keys matching any of these must
/// be dropped from child env and masked when shown to user/LLM.
const HIGH_RISK_ENV_PREFIXES: &[&str] = &[
    "AWS_",
    "GITHUB_",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_",
    "SLACK_",
    "STRIPE_",
    "STRIPE_SECRET",
    "DIGITALOCEAN_",
    "HEROKU_",
    "FIREBASE_",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "GOOGLE_API_KEY",
    "GCP_",
    "AZURE_",
    "TWILIO_",
    "SENDGRID_",
    "MAILGUN_",
    "NPM_TOKEN",
    "JEKYLL_",
    "RUBYGEMS_",
    "NODE_AUTH_TOKEN",
    "GH_TOKEN",
    "GL_TOKEN",
    "CI_JOB_TOKEN",
    "CI_REGISTRY_PASSWORD",
    "ACTIONS_RUNNER_",
];

/// Exact high-risk keys (case-insensitive match after normalizing to uppercase for comparison).
const HIGH_RISK_ENV_KEYS: &[&str] = &[
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "API_KEY",
    "SECRET_KEY",
    "PRIVATE_KEY",
    "ACCESS_KEY",
    "GITHUB_TOKEN",
    "GITLAB_TOKEN",
    "SLACK_TOKEN",
    "STRIPE_SECRET_KEY",
    "DB_PASSWORD",
    "DATABASE_URL", // often contains credentials
    "REDIS_URL",
    "AMQP_URL",
];

fn is_high_risk_key(key: &str) -> bool {
    let key_upper = key.to_uppercase();
    for prefix in HIGH_RISK_ENV_PREFIXES {
        if key_upper.starts_with(&prefix.to_uppercase()) {
            return true;
        }
    }
    for exact in HIGH_RISK_ENV_KEYS {
        if key_upper == *exact {
            return true;
        }
    }
    if key_upper.ends_with("_SECRET")
        || key_upper.ends_with("_TOKEN")
        || key_upper.ends_with("_PASSWORD")
        || key_upper.ends_with("_API_KEY")
        || key_upper.ends_with("_CREDENTIALS")
    {
        return true;
    }
    false
}

/// Builds a filtered environment for child processes: only keys that are NOT
/// high-risk are included. High-risk keys are dropped (not passed to the child).
/// Optionally annotate in logs that the process was started with a scrubbed env.
pub fn filter_env_for_child<I, K, V>(iter: I) -> HashMap<String, String>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    iter.into_iter()
        .filter(|(k, _)| !is_high_risk_key(k.as_ref()))
        .map(|(k, v)| (k.as_ref().to_string(), v.as_ref().to_string()))
        .collect()
}

/// Returns true if the current process env was filtered (any high-risk key was dropped).
/// Use when logging "scrubbed env" for child process start.
pub fn env_has_high_risk_keys() -> bool {
    std::env::vars().any(|(k, _)| is_high_risk_key(&k))
}

/// Build the minimal safe env for a child: filtered parent env plus required vars.
/// Overrides PATH, LANG, TERM with safe defaults if not provided.
pub fn safe_env_for_child(path: &str, lang: &str, term: &str) -> HashMap<String, String> {
    let mut env = filter_env_for_child(std::env::vars());
    env.insert("PATH".to_string(), path.to_string());
    env.insert("LANG".to_string(), lang.to_string());
    env.insert("TERM".to_string(), term.to_string());
    env
}

/// Format current environment for display (e.g. to user or agent), with secrets
/// scrubbed via the data_sensitivity classifier. Use this whenever env vars are
/// read to be shown; never expose raw env to the LLM or UI without scrubbing.
pub fn env_for_display() -> String {
    let raw: String = std::env::vars()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("\n");
    data_sensitivity::classify_and_mask_default(&raw).masked_content
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_risk_keys_dropped() {
        let env: Vec<(String, String)> = vec![
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("AWS_ACCESS_KEY_ID".to_string(), "secret".to_string()),
            ("OPENAI_API_KEY".to_string(), "sk-x".to_string()),
            ("HOME".to_string(), "/tmp".to_string()),
        ];
        let filtered = filter_env_for_child(env);
        assert!(filtered.contains_key("PATH"));
        assert!(filtered.contains_key("HOME"));
        assert!(!filtered.contains_key("AWS_ACCESS_KEY_ID"));
        assert!(!filtered.contains_key("OPENAI_API_KEY"));
    }

    #[test]
    fn suffix_patterns_dropped() {
        let env = vec![
            ("MY_SECRET".to_string(), "x".to_string()),
            ("DB_PASSWORD".to_string(), "x".to_string()),
        ];
        let filtered = filter_env_for_child(env);
        assert!(!filtered.contains_key("MY_SECRET"));
        assert!(!filtered.contains_key("DB_PASSWORD"));
    }

    #[test]
    fn synthetic_env_high_risk_absent_in_filtered() {
        let env: Vec<(String, String)> = vec![
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("HOME".to_string(), "/tmp".to_string()),
            ("AWS_ACCESS_KEY_ID".to_string(), "AKIAXXX".to_string()),
            ("AWS_SECRET_ACCESS_KEY".to_string(), "secret".to_string()),
            ("GITHUB_TOKEN".to_string(), "ghp_xxx".to_string()),
            ("OPENAI_API_KEY".to_string(), "sk-xxx".to_string()),
            ("SLACK_TOKEN".to_string(), "xoxb-xxx".to_string()),
            ("ANTHROPIC_API_KEY".to_string(), "sk-ant-xxx".to_string()),
        ];
        let filtered = filter_env_for_child(env);
        assert!(filtered.contains_key("PATH"));
        assert!(filtered.contains_key("HOME"));
        assert!(!filtered.contains_key("AWS_ACCESS_KEY_ID"));
        assert!(!filtered.contains_key("AWS_SECRET_ACCESS_KEY"));
        assert!(!filtered.contains_key("GITHUB_TOKEN"));
        assert!(!filtered.contains_key("OPENAI_API_KEY"));
        assert!(!filtered.contains_key("SLACK_TOKEN"));
        assert!(!filtered.contains_key("ANTHROPIC_API_KEY"));
    }
}
