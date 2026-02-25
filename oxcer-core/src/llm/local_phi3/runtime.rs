//! Phi-3 runtime abstraction.
//!
//! Implementations:
//! - [`StubPhiRuntime`] — canned response for tests and UI wiring.
//! - [`LlamaCppPhiRuntime`] — real GGUF inference via llama.cpp with Metal GPU offload.
//!
//! All inference is in-process; no HTTP, no localhost, no ports.
//!
//! # Why `llama-cpp-2` and not `llama_cpp 0.3`?
//!
//! `llama_cpp 0.3.x` bundles an old llama.cpp that only knows `LLM_ARCH_PHI2`.
//! Phi-3 (GGUF key `general.architecture = "phi3"`) was added in llama.cpp build
//! b2731 (May 2024). Attempting to load a phi3 GGUF with the old build triggers an
//! internal assertion. `llama-cpp-2 0.1` (utilityai) tracks llama.cpp closely and
//! includes full phi3 support.

use std::io::Read;
use std::num::NonZeroU32;
use std::path::Path;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;

use crate::llm::{GenerationParams, LlmError};

// ---------------------------------------------------------------------------
// Context / batch capacity constants
// ---------------------------------------------------------------------------

/// Hard context window (tokens) — must match the `with_n_ctx` value used in
/// `generate_direct`. Llama-3-8B-Instruct supports up to an 8 192-token window.
const CONTEXT_LIMIT: usize = 8192;

/// Tokens reserved at the end of the context for generation headroom.
/// Prevents the total sequence (prompt + generation) from silently overflowing
/// the KV cache by leaving a small cushion.
const GENERATION_SAFETY_MARGIN: usize = 64;

/// Returns the largest safe `max_tokens` that fits within the remaining context
/// after accounting for `prompt_tokens` and `GENERATION_SAFETY_MARGIN`.
///
/// Returns `Err` with a human-readable message when the prompt has already
/// consumed all available context, so callers can surface a clear
/// "prompt too long" message instead of letting llama.cpp fail with an
/// opaque batch error.
fn safe_max_tokens(prompt_tokens: usize, requested_max_tokens: usize) -> Result<usize, LlmError> {
    let available = CONTEXT_LIMIT
        .saturating_sub(prompt_tokens)
        .saturating_sub(GENERATION_SAFETY_MARGIN);
    if available == 0 {
        return Err(LlmError::GenerationFailed(format!(
            "prompt too long for context window: \
             prompt={prompt_tokens} tokens, context_limit={CONTEXT_LIMIT}, \
             safety_margin={GENERATION_SAFETY_MARGIN} — \
             no room left for generation. Try a shorter query."
        )));
    }
    Ok(requested_max_tokens.min(available))
}

/// Internal trait for Phi-3 inference. Engine calls this with token ids and params;
/// implementations can be stub, llama.cpp/GGUF, or ONNX Runtime.
pub trait PhiRuntime: Send + Sync {
    /// Generate token ids from input ids. Called by [super::LocalPhi3Engine] after tokenizing.
    /// Real runtimes (llama.cpp, ONNX) implement this method.
    fn generate(
        &self,
        input_ids: &[u32],
        params: &GenerationParams,
    ) -> Result<Vec<u32>, LlmError>;

    /// Optional direct-text path: return a complete response string, bypassing the
    /// tokenize → generate → detokenize round-trip.
    ///
    /// Return `Some(text)` to short-circuit the token path. Use this when the underlying
    /// inference library handles its own tokenization (e.g. llama_cpp's high-level API).
    /// Return `None` to fall through to the standard `generate` path.
    ///
    /// Default: `None`.
    fn generate_direct(
        &self,
        _prompt: &str,
        _params: &GenerationParams,
    ) -> Result<Option<String>, LlmError> {
        Ok(None)
    }
}

/// Stub runtime: canned response for tests.
///
/// `generate_direct` returns a visible canned response so the full UI path can be
/// exercised without running real inference.
#[allow(dead_code)]
pub struct StubPhiRuntime;

impl PhiRuntime for StubPhiRuntime {
    fn generate(
        &self,
        input_ids: &[u32],
        params: &GenerationParams,
    ) -> Result<Vec<u32>, LlmError> {
        let _ = (input_ids, params);
        Ok(vec![])
    }

    fn generate_direct(
        &self,
        _prompt: &str,
        _params: &GenerationParams,
    ) -> Result<Option<String>, LlmError> {
        Ok(Some(
            "(stub) Hello! Llama-3 inference via llama.cpp is not yet integrated. \
             The model file is present; wire a real PhiRuntime in LocalPhi3Engine::new to enable generation."
                .to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Local desktop-agent system prompt
// ---------------------------------------------------------------------------

/// System prompt injected as the `<|system|>` block for every inference call.
///
/// Policy:
/// - The model is an on-device assistant.  It is *allowed* to access the local
///   filesystem when the user asks.
/// - It must never say "I'm unable to directly view or access files on your
///   computer."  The tools `fs_list_dir` and `fs_read_file` exist for exactly
///   this purpose.
/// - When the user asks "what's in this folder?" or "summarise this file", the
///   model should direct the orchestrator to call `fs_list_dir` / `fs_read_file`
///   rather than declining.
const DESKTOP_AGENT_SYSTEM_PROMPT: &str = "\
You are Oxcer, a local desktop AI assistant running entirely on this machine.\n\
\n\
RULES — follow these without exception:\n\
1. If the user asks about the contents of a folder or file, you MUST use the \
corresponding filesystem tool (fs_list_dir for folders, fs_read_file for files) \
and base your answer solely on the tool result.\n\
2. Never invent, guess, or fabricate file names, folder structures, or file \
contents. If a tool fails or the path does not exist, say so explicitly.\n\
3. Never say \"I'm unable to directly view or access files on your computer\" — \
you have full access to the local filesystem through the tools below.\n\
4. When the user asks you to create, write, save, or summarise into a file:\n\
   a. Infer the target from prior tool outputs in this conversation — do NOT \
ask \"which file?\" if you already listed or read files in this session.\n\
   b. Choose a sensible output name (e.g. summary.md) if the user has not \
specified one.\n\
   c. Call fs_write_file to create the file, then report the path you wrote.\n\
   d. Only ask for clarification when there is genuine unresolvable ambiguity \
(e.g. two completely unrelated directories were listed and the intent truly \
cannot be determined from context).\n\
5. Never ask the user to repeat information that is already available from a \
prior tool result in the same conversation.\n\
\n\
Available filesystem tools:\n\
- fs_list_dir   : lists files and subdirectories in a folder. \
Use this when the user asks \"what's in this folder?\", \"show me the files\", \
\"list the directory\", or any similar request.\n\
- fs_read_file  : reads the text content of a file. \
Use this when the user asks to summarise, explain, review, or quote a file.\n\
- fs_write_file : creates or overwrites a file with text content. \
Use this when the user asks to \"make a summary\", \"write this to a file\", \
\"create summary.md\", or any similar write request. Infer a sensible file \
name from context if the user has not specified one — do not ask for the name.\n\
\n\
Example of the desired autonomous behaviour:\n\
  User: \"list my Desktop\"\n\
  Agent calls fs_list_dir and sees [notes.md, report.md, draft.md].\n\
  User: \"make summary.md with that\"\n\
  Agent calls fs_read_file on each .md file, then calls fs_write_file to write \
summary.md in the same directory. It does NOT ask \"which file did you want \
summarised?\" because the context is already clear from the prior tool output.\n\
\n\
When you need filesystem information, use the appropriate tool rather than \
inventing or declining.";

// ---------------------------------------------------------------------------
// Llama-3 prompt formatting
// ---------------------------------------------------------------------------

/// Build a Llama-3-instruct formatted prompt string.
///
/// Format (Meta instruct fine-tune):
/// ```text
/// <|begin_of_text|><|start_header_id|>system<|end_header_id|>
///
/// {system}<|eot_id|><|start_header_id|>user<|end_header_id|>
///
/// {user}<|eot_id|><|start_header_id|>assistant<|end_header_id|>
///
/// ```
///
/// Notes:
/// - `<|begin_of_text|>` is the BOS token for Llama-3 — include it explicitly in
///   the string and pass `AddBos::Never` to `str_to_token` to avoid double-BOS.
/// - Each header block is followed by two newlines before the content.
/// - The prompt ends immediately after the opening `<|start_header_id|>assistant<|end_header_id|>\n\n`
///   so the model continues from the assistant turn without a pre-written response.
fn build_llama3_prompt(system: &str, user: &str) -> String {
    format!(
        "<|begin_of_text|><|start_header_id|>system<|end_header_id|>\n\n\
         {system}<|eot_id|>\
         <|start_header_id|>user<|end_header_id|>\n\n\
         {user}<|eot_id|>\
         <|start_header_id|>assistant<|end_header_id|>\n\n"
    )
}

// ---------------------------------------------------------------------------
// LlamaCppPhiRuntime — real GGUF inference via llama-cpp-2
// ---------------------------------------------------------------------------

/// Real Phi-3 runtime using llama-cpp-2 (utilityai) with GGUF weights.
///
/// Uses `llama-cpp-2 0.1`, which bundles a modern llama.cpp with full phi3 support.
/// Offloads all transformer layers to Metal on Apple Silicon (`n_gpu_layers = 99`).
///
/// `generate_direct` is used so llama.cpp performs its own tokenisation from the
/// GGUF vocabulary — the HuggingFace tokenizer in `LocalPhi3Engine` is bypassed.
///
/// # Required model
///
/// A phi3-architecture GGUF, e.g. `Phi-3-mini-4k-instruct-Q4_K_M.gguf` from
/// `bartowski/Phi-3-mini-4k-instruct-GGUF` on HuggingFace.
pub struct LlamaCppPhiRuntime {
    // Dropped in declaration order: model freed before backend is torn down.
    model: LlamaModel,
    backend: LlamaBackend,
}

// SAFETY: LlamaModel wraps a raw llama.cpp model pointer. After load, model weights
// are read-only. generate_direct allocates an independent LlamaContext per call
// (independent KV cache), so concurrent calls from different threads are safe.
// LlamaBackend wraps llama.cpp global init state which is read-only after init.
unsafe impl Sync for LlamaCppPhiRuntime {}
unsafe impl Send for LlamaCppPhiRuntime {}

impl LlamaCppPhiRuntime {
    /// Load a phi3-architecture GGUF model and prepare for in-process Metal inference.
    ///
    /// Pre-flight checks:
    /// - Verifies the file exists and starts with the GGUF magic (`GGUF`).
    /// - Logs file name and size before calling into llama.cpp.
    ///
    /// `n_gpu_layers = 99` offloads all transformer layers to Metal (Apple Silicon).
    /// Set to `0` for CPU-only (slower, useful for debugging without a GPU).
    pub fn load(model_path: &Path) -> Result<Self, LlmError> {
        let file_name = model_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?");
        let file_size = std::fs::metadata(model_path)
            .map(|m| format!("{:.2} GiB", m.len() as f64 / (1u64 << 30) as f64))
            .unwrap_or_else(|_| "unknown size".into());

        log::info!(
            "[LlamaCppPhiRuntime] loading {} ({}) from {:?}",
            file_name,
            file_size,
            model_path
        );

        // Check GGUF magic before calling llama.cpp so we get a clear error instead
        // of an opaque assertion failure.
        check_gguf_magic(model_path)?;

        // Initialise llama.cpp global state (Metal device discovery, thread pools, etc.).
        // Stored in struct so it lives exactly as long as the model.
        let backend = LlamaBackend::init().map_err(|e| {
            LlmError::Config(format!("llama.cpp backend init failed: {e}"))
        })?;

        let model_params = LlamaModelParams::default()
            .with_n_gpu_layers(99); // 0 = CPU-only (for debugging)

        let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
            .map_err(|e| {
                LlmError::Config(format!(
                    "GGUF load failed for {:?}: {}\n\
                     Ensure the file is a Llama-3 GGUF \
                     (e.g. Meta-Llama-3-8B-Instruct-Q4_K_M.gguf from \
                     bartowski/Meta-Llama-3-8B-Instruct-GGUF on HuggingFace).",
                    model_path, e
                ))
            })?;

        log::info!(
            "[LlamaCppPhiRuntime] ready — {} params, n_gpu_layers=99",
            model.n_params()
        );

        Ok(Self { model, backend })
    }
}

/// Verify the file is readable and starts with the four-byte GGUF magic.
///
/// Returns a human-readable `LlmError::Config` instead of the opaque llama.cpp
/// assertion message when the file is absent, unreadable, or wrong format.
fn check_gguf_magic(path: &Path) -> Result<(), LlmError> {
    let mut f = std::fs::File::open(path).map_err(|e| {
        LlmError::Config(format!(
            "Cannot open model file {:?}: {}\n\
             Make sure the model has been downloaded to the correct location.",
            path, e
        ))
    })?;

    let mut magic = [0u8; 4];
    f.read_exact(&mut magic).map_err(|e| {
        LlmError::Config(format!("Cannot read model file {:?}: {}", path, e))
    })?;

    if &magic != b"GGUF" {
        return Err(LlmError::Config(format!(
            "File {:?} is not a valid GGUF (got magic {:?}, expected b\"GGUF\"). \
             Download a .gguf model file.",
            path, magic
        )));
    }

    Ok(())
}

impl PhiRuntime for LlamaCppPhiRuntime {
    /// Not used: `generate_direct` takes priority for this runtime.
    fn generate(
        &self,
        _input_ids: &[u32],
        _params: &GenerationParams,
    ) -> Result<Vec<u32>, LlmError> {
        Ok(vec![])
    }

    /// Run inference using llama.cpp's internal tokeniser and sampler.
    ///
    /// Creates a fresh `LlamaContext` per call (independent KV cache, no cross-request
    /// state leakage). Stops on EOS token, any stop sequence from `params`, or after
    /// `params.max_tokens` new tokens.
    ///
    /// Sampler chain: stochastic distribution (seed 42) → greedy pick.
    fn generate_direct(
        &self,
        prompt: &str,
        params: &GenerationParams,
    ) -> Result<Option<String>, LlmError> {
        // Apply the Llama-3 instruct chat template before tokenising.
        //
        // Llama-3-8B-Instruct expects the Meta instruct format:
        //   <|begin_of_text|><|start_header_id|>system<|end_header_id|>\n\n
        //   {system}<|eot_id|><|start_header_id|>user<|end_header_id|>\n\n
        //   {user}<|eot_id|><|start_header_id|>assistant<|end_header_id|>\n\n
        //
        // <|begin_of_text|> is Llama-3's BOS token — it is included explicitly in the
        // formatted string, so we pass AddBos::Never to avoid double-BOS insertion.
        //
        // Without the template the model runs in raw continuation mode.
        let formatted = build_llama3_prompt(DESKTOP_AGENT_SYSTEM_PROMPT, prompt);

        log::info!(
            "[LlamaCppPhiRuntime] generate_direct engine=LlamaCppPhiRuntime \
             raw_len={} formatted_len={}",
            prompt.len(),
            formatted.len()
        );
        log::debug!(
            "[LlamaCppPhiRuntime] formatted_prompt='{}'",
            formatted.chars().take(200).collect::<String>()
        );

        // Fresh context per call — independent KV cache, no cross-request state.
        // Llama-3-8B-Instruct supports an 8 192-token context window.
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(Some(NonZeroU32::new(8192).unwrap()));

        let mut ctx = self
            .model
            .new_context(&self.backend, ctx_params)
            .map_err(|e| {
                LlmError::GenerationFailed(format!("create_context failed: {e}"))
            })?;

        // Tokenise using llama.cpp's internal GGUF vocabulary (not the HuggingFace tokenizer).
        // AddBos::Never — <|begin_of_text|> is already the first token in `formatted`.
        let tokens = self
            .model
            .str_to_token(&formatted, AddBos::Never)
            .map_err(|e| {
                LlmError::GenerationFailed(format!("tokenisation failed: {e}"))
            })?;

        if tokens.is_empty() {
            return Ok(Some(String::new()));
        }

        let prompt_tokens = tokens.len();

        // Pre-flight: verify the prompt fits within the context window and compute
        // how many new tokens we can safely generate before hitting the KV-cache limit.
        // This converts the opaque "Insufficient Space" llama.cpp error into a clear,
        // actionable message before any allocation is made.
        let safe_max = safe_max_tokens(prompt_tokens, params.max_tokens)?;

        tracing::info!(
            event = "llm_preflight",
            prompt_tokens = prompt_tokens,
            safe_max_tokens = safe_max,
            context_limit = CONTEXT_LIMIT,
            "prefill ready"
        );

        // Prefill: decode all prompt tokens in one batch to populate the KV cache.
        //
        // The batch capacity is sized dynamically to the actual prompt length instead
        // of a hard-coded 512 — the previous 512 caused "Insufficient Space" failures
        // whenever the system prompt + directory listing + user query exceeded 512 tokens.
        let batch_cap = prompt_tokens; // usize, matches LlamaBatch::new signature
        let mut batch = LlamaBatch::new(batch_cap, 1);
        let last_idx = (prompt_tokens - 1) as i32;
        for (i, &token) in tokens.iter().enumerate() {
            let is_last = i as i32 == last_idx;
            batch.add(token, i as i32, &[0], is_last).map_err(|e| {
                LlmError::GenerationFailed(format!(
                    "batch.add (prefill) failed at token {i}/{prompt_tokens}: {e} \
                     (batch_cap={batch_cap}, context_limit={CONTEXT_LIMIT})"
                ))
            })?;
        }
        ctx.decode(&mut batch).map_err(|e| {
            LlmError::GenerationFailed(format!(
                "prefill decode failed: {e} \
                 (prompt_tokens={prompt_tokens}, context_limit={CONTEXT_LIMIT})"
            ))
        })?;

        // Generation loop — one token at a time.
        let mut n_cur = batch.n_tokens();
        // Use safe_max (not params.max_tokens) to stay within the context window.
        let max_new = safe_max as i32;

        let mut sampler = LlamaSampler::chain_simple([
            LlamaSampler::dist(42), // stochastic distribution (temperature/top-k/top-p)
            LlamaSampler::greedy(), // pick the highest-probability token
        ]);

        let mut utf8_dec = encoding_rs::UTF_8.new_decoder();
        let mut text = String::new();

        while n_cur < (tokens.len() as i32).saturating_add(max_new) {
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            sampler.accept(token);

            if self.model.is_eog_token(token) {
                break;
            }

            let piece = self
                .model
                .token_to_piece(token, &mut utf8_dec, true, None)
                .map_err(|e| {
                    LlmError::GenerationFailed(format!("token_to_piece failed: {e}"))
                })?;
            text.push_str(&piece);

            // Stop-sequence check: truncate and stop if any sequence is matched.
            let mut stopped = false;
            for stop in &params.stop_sequences {
                if let Some(pos) = text.find(stop.as_str()) {
                    text.truncate(pos);
                    stopped = true;
                    break;
                }
            }
            if stopped {
                break;
            }

            batch.clear();
            batch
                .add(token, n_cur, &[0], true)
                .map_err(|e| {
                    LlmError::GenerationFailed(format!("batch.add (gen) failed: {e}"))
                })?;
            n_cur += 1;

            ctx.decode(&mut batch).map_err(|e| {
                LlmError::GenerationFailed(format!("generation decode failed: {e}"))
            })?;
        }

        let tokens_generated = (n_cur - prompt_tokens as i32).max(0) as usize;
        tracing::info!(
            event = "llm_generate_done",
            prompt_tokens = prompt_tokens,
            tokens_generated = tokens_generated,
            text_len = text.len(),
            context_limit = CONTEXT_LIMIT,
            "generate_direct done"
        );
        log::debug!(
            "[LlamaCppPhiRuntime] done tokens_generated={} text_len={} preview='{}'",
            tokens_generated,
            text.len(),
            text.chars().take(120).collect::<String>()
        );

        Ok(Some(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_max_tokens_normal_prompt_clamps_to_requested() {
        // 200 tokens prompt, 512 requested → 512 fits easily within 4096 - 64 headroom.
        let result = safe_max_tokens(200, 512).unwrap();
        assert_eq!(result, 512);
    }

    #[test]
    fn safe_max_tokens_large_request_clamped_to_available() {
        // 200 tokens prompt, 9999 requested → available = 8192 - 200 - 64 = 7928.
        let result = safe_max_tokens(200, 9999).unwrap();
        assert_eq!(result, 8192 - 200 - 64);
    }

    #[test]
    fn safe_max_tokens_tight_prompt_leaves_one_token() {
        // prompt fills context except for the safety margin + 1 token.
        let prompt_tokens = CONTEXT_LIMIT - GENERATION_SAFETY_MARGIN - 1;
        let result = safe_max_tokens(prompt_tokens, 512).unwrap();
        assert_eq!(result, 1);
    }

    #[test]
    fn safe_max_tokens_at_limit_returns_err() {
        // prompt exactly fills context minus safety margin → available == 0.
        let prompt_tokens = CONTEXT_LIMIT - GENERATION_SAFETY_MARGIN;
        let err = safe_max_tokens(prompt_tokens, 512).unwrap_err();
        assert!(err.to_string().contains("prompt too long"));
    }

    #[test]
    fn safe_max_tokens_over_limit_returns_err() {
        let err = safe_max_tokens(CONTEXT_LIMIT + 100, 512).unwrap_err();
        assert!(err.to_string().contains("prompt too long"));
    }
}
