# Phi-3-small (local)

Place the following files here for the in-process local LLM engine:

- `model.gguf` — model weights (e.g. from Hugging Face `microsoft/Phi-3-small-128k-instruct`)
- `tokenizer.json` — tokenizer

Configure in `oxcer-core/config/models.yaml`. Model download can be implemented in `oxcer-core` to fetch these automatically.
