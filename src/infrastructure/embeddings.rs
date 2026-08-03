use sha2::{Digest, Sha256};

use crate::{
    application::retrieval::{
        DEFAULT_EMBEDDING_DIMENSION, DEFAULT_EMBEDDING_MODEL_ID, EmbeddingProvider,
    },
    error::{LoreError, Result},
};

/// Small, deterministic, dependency-free embedding provider used by the local MVP.
///
/// It combines normalized terms and character trigrams with signed feature hashing. It is
/// intentionally replaceable through `EmbeddingProvider`; it does not download a model or
/// require a network service, while still giving the hybrid ranker a stable semantic signal.
#[derive(Debug, Clone, Copy, Default)]
pub struct HashEmbeddingProvider;

impl HashEmbeddingProvider {
    pub const fn new() -> Self {
        Self
    }
}

impl EmbeddingProvider for HashEmbeddingProvider {
    fn model_id(&self) -> &str {
        DEFAULT_EMBEDDING_MODEL_ID
    }

    fn dimension(&self) -> usize {
        DEFAULT_EMBEDDING_DIMENSION
    }

    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let mut vector = vec![0.0; self.dimension()];
        let mut feature_count = 0_u32;
        for token in normalized_tokens(text) {
            add_feature(&mut vector, format!("term:{token}").as_bytes(), 1.0);
            feature_count += 1;
            let bytes = token.as_bytes();
            for trigram in bytes.windows(3) {
                add_feature(&mut vector, trigram, 0.25);
                feature_count += 1;
            }
        }
        if feature_count == 0 {
            return Err(LoreError::Configuration(
                "embedding input does not contain searchable terms".into(),
            ));
        }
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        if norm == 0.0 || !norm.is_finite() {
            return Err(LoreError::Configuration(
                "embedding vector could not be normalized".into(),
            ));
        }
        for value in &mut vector {
            *value /= norm;
        }
        Ok(vector)
    }
}

fn normalized_tokens(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|token| token.len() >= 2)
        .map(|token| canonical_term(&token.to_ascii_lowercase()))
        .filter(|token| !token.is_empty())
        .collect()
}

fn canonical_term(token: &str) -> String {
    match token {
        "authentication" | "authenticate" | "auth" | "login" | "signin" | "signing" => {
            "auth".into()
        }
        "authorization" | "authorize" | "permission" | "permissions" | "access" => {
            "authorization".into()
        }
        "bug" | "bugs" | "issue" | "issues" | "problem" | "problems" | "error" | "errors"
        | "falha" | "falhas" => "issue".into(),
        "fix" | "fixed" | "fixes" | "fixing" | "resolve" | "resolved" | "resolution" | "repair"
        | "repairing" | "corrigir" | "correcao" => "fix".into(),
        "test" | "tests" | "testing" | "validated" | "validation" | "validate" | "teste"
        | "testes" | "validacao" => "test".into(),
        "cache" | "cached" | "caching" => "cache".into(),
        "database" | "databases" | "db" | "sqlite" | "postgres" => "database".into(),
        "performance" | "latency" | "slow" | "slowness" => "performance".into(),
        "credential" | "credentials" | "secret" | "secrets" | "token" | "tokens" => {
            "credential".into()
        }
        _ => token.to_owned(),
    }
}

fn add_feature(vector: &mut [f32], feature: &[u8], weight: f32) {
    let mut hasher = Sha256::new();
    hasher.update(feature);
    let digest = hasher.finalize();
    let index = u16::from_le_bytes([digest[0], digest[1]]) as usize % vector.len();
    let sign = if digest[2] & 1 == 0 { 1.0 } else { -1.0 };
    vector[index] += sign * weight;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::retrieval::EmbeddingProvider;

    #[test]
    fn provider_is_deterministic_and_normalized() {
        let provider = HashEmbeddingProvider::new();
        let first = provider
            .embed("fix authentication issue")
            .expect("embedding");
        let second = provider
            .embed("fix authentication issue")
            .expect("embedding");
        assert_eq!(first, second);
        assert_eq!(first.len(), DEFAULT_EMBEDDING_DIMENSION);
        let norm = first.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.0001);
    }

    #[test]
    fn provider_bridges_small_paraphrases() {
        let provider = HashEmbeddingProvider::new();
        let first = provider
            .embed("resolve authentication errors")
            .expect("embedding");
        let second = provider.embed("fix auth issue").expect("embedding");
        let similarity = first
            .iter()
            .zip(second)
            .map(|(left, right)| left * right)
            .sum::<f32>();
        assert!(similarity > 0.25);
    }
}
