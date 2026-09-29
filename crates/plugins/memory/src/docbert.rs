//! docbert's ColBERT model as an [`Encoder`]: `lightonai/GTE-ModernColBERT-v1`
//! by default, through [`docbert_pylate`], loaded on first use and
//! downloaded from the Hugging Face hub into its usual cache when missing.
//! Queries get no prompt, as in docbert, so the two rank alike.

use candle_core::Device;
use docbert_pylate::ColBERT;

use crate::colbert::{Encoder, Tokens};

/// docbert's default model.
pub const MODEL: &str = "lightonai/GTE-ModernColBERT-v1";

pub struct Docbert {
    model_id: String,
    model: Option<ColBERT>,
}

impl Docbert {
    pub fn new() -> Self {
        Self::with_model(MODEL)
    }

    pub fn with_model(model_id: impl Into<String>) -> Self {
        Self {
            model_id: model_id.into(),
            model: None,
        }
    }

    fn model(&mut self) -> anyhow::Result<&mut ColBERT> {
        if self.model.is_none() {
            let model: ColBERT = ColBERT::from(&self.model_id)
                .with_device(Device::Cpu)
                .with_query_prompt(String::new())
                .try_into()?;
            self.model = Some(model);
        }
        Ok(self.model.as_mut().expect("loaded above"))
    }
}

impl Default for Docbert {
    fn default() -> Self {
        Self::new()
    }
}

impl Encoder for Docbert {
    fn documents(&mut self, texts: &[String]) -> anyhow::Result<Vec<Tokens>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let (tensor, lengths) =
            self.model()?.encode_documents_with_lengths(texts)?;
        // `[texts, padded tokens, dim]`: each text's first `length` rows
        // are its tokens, the rest padding.
        let rows = tensor.to_vec3::<f32>()?;
        Ok(rows
            .into_iter()
            .zip(lengths)
            .map(|(rows, length)| {
                let dim = rows.first().map_or(0, Vec::len);
                Tokens {
                    dim,
                    values: rows
                        .into_iter()
                        .take(length as usize)
                        .flatten()
                        .collect(),
                }
            })
            .collect())
    }

    fn query(&mut self, text: &str) -> anyhow::Result<Tokens> {
        let tensor = self.model()?.encode(&[text.to_owned()], true)?;
        let rows = tensor.squeeze(0)?.to_vec2::<f32>()?;
        Ok(Tokens {
            dim: rows.first().map_or(0, Vec::len),
            values: rows.into_iter().flatten().collect(),
        })
    }

    fn model(&self) -> &str {
        &self.model_id
    }
}
