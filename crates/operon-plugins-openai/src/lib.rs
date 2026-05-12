//! Operon ChatPlugin for the OpenAI Chat Completions and Responses APIs.
//!
//! Also serves OpenAI-compatible endpoints (Ollama, vLLM, llama.cpp server, LM Studio)
//! via the `api_url` field on `OpenAIConfig`.

#[cfg(not(target_arch = "wasm32"))]
pub mod openai;
#[cfg(not(target_arch = "wasm32"))]
pub mod sse;

#[cfg(not(target_arch = "wasm32"))]
pub use openai::{OpenAIChatPlugin, OpenAIConfig};
