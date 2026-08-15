use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ort::session::Session;
use ort::value::Tensor;
use tokenizers::utils::padding::{PaddingParams, PaddingStrategy};
use tokenizers::utils::truncation::TruncationParams;
use tokenizers::Tokenizer;

use crate::config::Config;
use crate::index::EMBEDDING_DIM;

const MODEL_URL: &str =
    "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx";
const TOKENIZER_URL: &str =
    "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json";
const MAX_TOKENS: usize = 256;

#[derive(Clone)]
pub struct Embedder {
    session: Arc<Mutex<Session>>,
    tokenizer: Tokenizer,
}

impl Embedder {
    pub fn load(model_dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(model_dir)?;
        let model_path = model_dir.join("model.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");
        download_if_missing(MODEL_URL, &model_path)?;
        download_if_missing(TOKENIZER_URL, &tokenizer_path)?;

        let session = Session::builder()?.commit_from_file(&model_path)?;
        let mut tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("failed to load tokenizer: {e}"))?;
        let _ = tokenizer.with_truncation(Some(TruncationParams {
            max_length: MAX_TOKENS,
            ..Default::default()
        }));
        let _ = tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            ..Default::default()
        }));
        Ok(Self {
            session: Arc::new(Mutex::new(session)),
            tokenizer,
        })
    }

    pub fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let clean: Vec<String> = texts
            .iter()
            .map(|t| {
                if t.trim().is_empty() {
                    " ".to_string()
                } else {
                    t.clone()
                }
            })
            .collect();
        let encodings = self
            .tokenizer
            .encode_batch(clean, true)
            .map_err(|e| anyhow::anyhow!("tokenization failed: {e}"))?;

        let batch = encodings.len();
        let seq_len = encodings[0].len();
        let mut ids = Vec::with_capacity(batch * seq_len);
        let mut mask = Vec::with_capacity(batch * seq_len);
        let mut type_ids = Vec::with_capacity(batch * seq_len);
        for encoding in &encodings {
            ids.extend(encoding.get_ids().iter().map(|&v| v as i64));
            mask.extend(encoding.get_attention_mask().iter().map(|&v| v as i64));
            type_ids.extend(encoding.get_type_ids().iter().map(|&v| v as i64));
        }

        let mut session = self.session.lock().unwrap();
        let outputs = session.run(ort::inputs! {
            "input_ids" => Tensor::from_array(([batch, seq_len], ids))?,
            "attention_mask" => Tensor::from_array(([batch, seq_len], mask.clone()))?,
            "token_type_ids" => Tensor::from_array(([batch, seq_len], type_ids))?,
        })?;

        match outputs.get("last_hidden_state") {
            Some(value) => {
                let (shape, hidden) = value.try_extract_tensor::<f32>()?;
                let hidden_dim = shape[2] as usize;
                Ok(mean_pool(hidden, &mask, batch, seq_len, hidden_dim))
            }
            None => {
                let value = outputs
                    .get("sentence_embedding")
                    .ok_or_else(|| anyhow::anyhow!("model output not found"))?;
                let (shape, embeddings) = value.try_extract_tensor::<f32>()?;
                let dim = shape[1] as usize;
                Ok(embeddings.chunks(dim).map(l2_normalize).collect())
            }
        }
    }
}

fn mean_pool(hidden: &[f32], mask: &[i64], batch: usize, seq_len: usize, dim: usize) -> Vec<Vec<f32>> {
    (0..batch)
        .map(|b| {
            let mut sum = vec![0.0f32; dim];
            let mut count = 0.0f32;
            for t in 0..seq_len {
                if mask[b * seq_len + t] != 0 {
                    let offset = (b * seq_len + t) * dim;
                    for (d, value) in sum.iter_mut().enumerate() {
                        *value += hidden[offset + d];
                    }
                    count += 1.0;
                }
            }
            l2_normalize(&sum.into_iter().map(|v| v / count.max(1.0)).collect::<Vec<f32>>())
        })
        .collect()
}

fn l2_normalize(vector: &[f32]) -> Vec<f32> {
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
    vector.iter().map(|v| v / norm).collect()
}

fn download_if_missing(url: &str, dest: &Path) -> anyhow::Result<()> {
    if dest.exists() {
        return Ok(());
    }
    let tmp = dest.with_extension(format!("part-{}", std::process::id()));
    let response = ureq::get(url)
        .timeout(Duration::from_secs(600))
        .call()
        .map_err(|e| anyhow::anyhow!("failed to download {url}: {e}"))?;
    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(&tmp)?;
    std::io::copy(&mut reader, &mut file)?;
    match std::fs::rename(&tmp, dest) {
        Ok(()) => Ok(()),
        Err(_) if dest.exists() => {
            let _ = std::fs::remove_file(&tmp);
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    #[test]
    fn embeds_with_semantic_similarity() {
        let embedder = Embedder::load(&Config::from_env().model_dir).unwrap();
        let texts = vec![
            "The borrow checker enforces memory safety in Rust programs.".to_string(),
            "Rust ownership prevents data races at compile time.".to_string(),
            "Bananas are a good source of potassium and fiber.".to_string(),
        ];
        let vectors = embedder.embed(&texts).unwrap();
        assert_eq!(vectors.len(), 3);
        assert!(vectors.iter().all(|v| v.len() == EMBEDDING_DIM));
        assert!(vectors.iter().all(|v| (v.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-3));

        let rust_related = cosine(&vectors[0], &vectors[1]);
        let unrelated = cosine(&vectors[0], &vectors[2]);
        assert!(rust_related > unrelated);
    }

    #[test]
    fn embeds_empty_inputs_gracefully() {
        let embedder = Embedder::load(&Config::from_env().model_dir).unwrap();
        let vectors = embedder.embed(&["  ".to_string()]).unwrap();
        assert_eq!(vectors.len(), 1);
        assert_eq!(vectors[0].len(), EMBEDDING_DIM);
        assert!(embedder.embed(&[]).unwrap().is_empty());
    }
}
