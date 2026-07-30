//! Orca-compatible local speech model catalog, cache, download, and inference.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use bzip2::read::BzDecoder;
use futures::StreamExt;
use sha2::{Digest, Sha256};
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig,
    OfflineWhisperModelConfig, OnlineParaformerModelConfig, OnlineRecognizer,
    OnlineRecognizerConfig, OnlineTransducerModelConfig,
};
use tokio::io::AsyncWriteExt;

use crate::speech::AudioCapture;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    OfflineTransducer,
    OnlineTransducer,
    OnlineParaformer,
    Whisper,
}

#[derive(Debug, Clone, Copy)]
pub struct SpeechModelManifest {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub kind: ModelKind,
    pub size_bytes: u64,
    pub download_url: &'static str,
    pub archive_sha256: &'static str,
    pub files: &'static [&'static str],
    pub modeling_unit: Option<&'static str>,
}

pub const LOCAL_MODELS: &[SpeechModelManifest] = &[
    SpeechModelManifest {
        id: "parakeet-tdt-0.6b-v3-int8",
        label: "Parakeet TDT v3",
        description: "Highest accuracy for 25 European languages. Punctuation, capitalization, and word-level timestamps.",
        kind: ModelKind::OfflineTransducer,
        size_bytes: 487_170_055,
        download_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
        archive_sha256: "5793d0fd397c5778d2cf2126994d58e9d56b1be7c04d13c7a15bb1b4eafb16bf",
        files: &["encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"],
        modeling_unit: Some("bpe"),
    },
    SpeechModelManifest {
        id: "parakeet-tdt-0.6b-v2-int8",
        label: "Parakeet TDT v2",
        description: "English only. Faster than v3 with similar accuracy. Punctuation and capitalization.",
        kind: ModelKind::OfflineTransducer,
        size_bytes: 482_468_385,
        download_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2",
        archive_sha256: "157c157bc51155e03e37d2466522a3a737dd9c72bb25f36eb18912964161e1ad",
        files: &["encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"],
        modeling_unit: Some("bpe"),
    },
    SpeechModelManifest {
        id: "zipformer-bilingual-zh-en",
        label: "Zipformer Bilingual",
        description: "Chinese + English with code-switching. Low-latency real-time streaming.",
        kind: ModelKind::OnlineTransducer,
        size_bytes: 511_274_346,
        download_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20.tar.bz2",
        archive_sha256: "27ffbd9ee24ad186d99acc2f6354d7992b27bcab490812510665fa8f9389c5f8",
        files: &["encoder-epoch-99-avg-1.onnx", "decoder-epoch-99-avg-1.onnx", "joiner-epoch-99-avg-1.onnx", "tokens.txt"],
        modeling_unit: Some("cjkchar+bpe"),
    },
    SpeechModelManifest {
        id: "paraformer-bilingual-zh-en",
        label: "Paraformer Bilingual",
        description: "Chinese (Mandarin + dialects) + English. Strong on accented and regional Chinese.",
        kind: ModelKind::OnlineParaformer,
        size_bytes: 1_047_319_737,
        download_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-paraformer-bilingual-zh-en.tar.bz2",
        archive_sha256: "5462a1fce42693deae572af1e8c4687124b12aa85fe61ff4d3168bb5280e205f",
        files: &["encoder.int8.onnx", "decoder.int8.onnx", "tokens.txt"],
        modeling_unit: None,
    },
    SpeechModelManifest {
        id: "zipformer-streaming-en-20m",
        label: "Zipformer Streaming EN",
        description: "English only. Lightweight 20M-param model, good balance of speed and size.",
        kind: ModelKind::OnlineTransducer,
        size_bytes: 127_887_156,
        download_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17.tar.bz2",
        archive_sha256: "9c559283e8498d3fe95913c79ca1cb454bb26281ac2b102b41306c7d752765d9",
        files: &["encoder-epoch-99-avg-1.onnx", "decoder-epoch-99-avg-1.onnx", "joiner-epoch-99-avg-1.onnx", "tokens.txt"],
        modeling_unit: Some("bpe"),
    },
    SpeechModelManifest {
        id: "zipformer-streaming-zh-14m",
        label: "Zipformer Streaming ZH",
        description: "Chinese only. Ultra-lightweight 14M-param model, ideal for low-resource devices.",
        kind: ModelKind::OnlineTransducer,
        size_bytes: 74_004_050,
        download_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-zh-14M-2023-02-23.tar.bz2",
        archive_sha256: "2cbd71b640d9c37d3784f29367333a4577b0398b62e9deeed418170b081cba8b",
        files: &["encoder-epoch-99-avg-1.onnx", "decoder-epoch-99-avg-1.onnx", "joiner-epoch-99-avg-1.onnx", "tokens.txt"],
        modeling_unit: Some("cjkchar"),
    },
    SpeechModelManifest {
        id: "whisper-tiny",
        label: "Whisper Tiny",
        description: "90+ languages. Lower accuracy than Parakeet but broadest language coverage.",
        kind: ModelKind::Whisper,
        size_bytes: 116_204_861,
        download_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-whisper-tiny.tar.bz2",
        archive_sha256: "c46116994e539aa165266d96b325252728429c12535eb9d8b6a2b10f129e66b1",
        files: &["tiny-encoder.onnx", "tiny-decoder.onnx", "tiny-tokens.txt"],
        modeling_unit: None,
    },
];

pub fn local_model(id: &str) -> Option<&'static SpeechModelManifest> {
    LOCAL_MODELS.iter().find(|model| model.id == id)
}

pub fn models_dir(custom: &str) -> PathBuf {
    if !custom.trim().is_empty() {
        return PathBuf::from(custom.trim());
    }
    crate::persistence_thread::default_data_file()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("speech-models")
}

pub fn model_dir(model_id: &str, custom_models_dir: &str) -> Option<PathBuf> {
    local_model(model_id).map(|_| models_dir(custom_models_dir).join(model_id))
}

pub fn is_ready(model_id: &str, custom_models_dir: &str) -> bool {
    let Some(manifest) = local_model(model_id) else {
        return false;
    };
    let Some(dir) = model_dir(model_id, custom_models_dir) else {
        return false;
    };
    manifest.files.iter().all(|file| dir.join(file).is_file())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    let digest = hash.finalize();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn directory_with_files(
    root: &Path,
    files: &[&str],
    depth: usize,
) -> Result<Option<PathBuf>, String> {
    if files.iter().all(|file| root.join(file).is_file()) {
        return Ok(Some(root.to_path_buf()));
    }
    if depth == 0 {
        return Ok(None);
    }
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            if let Some(found) = directory_with_files(&entry.path(), files, depth - 1)? {
                return Ok(Some(found));
            }
        }
    }
    Ok(None)
}

fn extract_archive(
    archive_path: &Path,
    target_dir: &Path,
    manifest: &SpeechModelManifest,
) -> Result<(), String> {
    let root = target_dir
        .parent()
        .ok_or_else(|| "Speech model cache has no parent directory.".to_string())?;
    let staging = tempfile::tempdir_in(root).map_err(|error| error.to_string())?;
    let file = File::open(archive_path).map_err(|error| error.to_string())?;
    let mut archive = tar::Archive::new(BzDecoder::new(file));
    archive
        .unpack(staging.path())
        .map_err(|error| format!("Could not extract speech model: {error}"))?;
    let source = directory_with_files(staging.path(), manifest.files, 4)?
        .ok_or_else(|| "Model files are missing after extraction.".to_string())?;
    if target_dir.exists() {
        fs::remove_dir_all(target_dir).map_err(|error| error.to_string())?;
    }
    if source == staging.path() {
        fs::create_dir_all(target_dir).map_err(|error| error.to_string())?;
        for entry in fs::read_dir(&source).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let destination = target_dir.join(entry.file_name());
            fs::rename(entry.path(), destination).map_err(|error| error.to_string())?;
        }
    } else {
        fs::rename(source, target_dir).map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub async fn download_model(model_id: String, custom_models_dir: String) -> Result<(), String> {
    let manifest = local_model(&model_id).ok_or_else(|| format!("Unknown model: {model_id}"))?;
    let root = models_dir(&custom_models_dir);
    tokio::fs::create_dir_all(&root)
        .await
        .map_err(|error| format!("Could not create speech model cache: {error}"))?;
    if is_ready(&model_id, &custom_models_dir) {
        return Ok(());
    }
    let archive_path = root.join(format!("{model_id}.tar.bz2"));
    let target_dir = root.join(&model_id);
    let result = async {
        let response = reqwest::Client::new()
            .get(manifest.download_url)
            .send()
            .await
            .map_err(|error| format!("Speech model download failed: {error}"))?
            .error_for_status()
            .map_err(|error| format!("Speech model download failed: {error}"))?;
        let mut file = tokio::fs::File::create(&archive_path)
            .await
            .map_err(|error| format!("Could not create model archive: {error}"))?;
        let mut stream = response.bytes_stream();
        let mut downloaded = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| format!("Speech model download failed: {error}"))?;
            file.write_all(&chunk)
                .await
                .map_err(|error| format!("Could not write model archive: {error}"))?;
            downloaded += chunk.len() as u64;
        }
        file.flush().await.map_err(|error| error.to_string())?;
        drop(file);
        if downloaded != manifest.size_bytes {
            return Err(format!(
                "Speech model download was incomplete (expected {} bytes, received {downloaded}).",
                manifest.size_bytes
            ));
        }
        let verify_path = archive_path.clone();
        let actual_hash = tokio::task::spawn_blocking(move || sha256_file(&verify_path))
            .await
            .map_err(|error| error.to_string())??;
        if actual_hash != manifest.archive_sha256 {
            return Err("Speech model archive failed SHA-256 verification.".to_string());
        }
        let archive = archive_path.clone();
        let target = target_dir.clone();
        tokio::task::spawn_blocking(move || extract_archive(&archive, &target, manifest))
            .await
            .map_err(|error| error.to_string())??;
        if !is_ready(&model_id, &custom_models_dir) {
            return Err("Speech model files are missing after extraction.".to_string());
        }
        Ok(())
    }
    .await;
    let _ = tokio::fs::remove_file(&archive_path).await;
    if result.is_err() {
        let _ = tokio::fs::remove_dir_all(&target_dir).await;
    }
    result
}

pub async fn delete_model(model_id: String, custom_models_dir: String) -> Result<(), String> {
    let dir = model_dir(&model_id, &custom_models_dir)
        .ok_or_else(|| format!("Unknown model: {model_id}"))?;
    if dir.exists() {
        tokio::fs::remove_dir_all(dir)
            .await
            .map_err(|error| format!("Could not delete speech model: {error}"))?;
    }
    Ok(())
}

fn resolve_file(dir: &Path, files: &[&str], role: &str) -> Result<String, String> {
    files
        .iter()
        .find(|file| file.contains(role) && file.ends_with(".onnx"))
        .map(|file| dir.join(file).to_string_lossy().into_owned())
        .ok_or_else(|| format!("No *{role}*.onnx file exists in the speech model."))
}

fn resolve_tokens(dir: &Path, files: &[&str]) -> Result<String, String> {
    files
        .iter()
        .find(|file| file.ends_with("tokens.txt"))
        .map(|file| dir.join(file).to_string_lossy().into_owned())
        .ok_or_else(|| "No tokens.txt file exists in the speech model.".to_string())
}

fn decode_offline(
    capture: &AudioCapture,
    manifest: &SpeechModelManifest,
    dir: &Path,
    language: &str,
) -> Result<String, String> {
    let mut config = OfflineRecognizerConfig::default();
    config.feat_config.sample_rate = 16_000;
    config.feat_config.feature_dim = 80;
    config.model_config.tokens = Some(resolve_tokens(dir, manifest.files)?);
    config.model_config.num_threads = 2;
    config.model_config.provider = Some("cpu".into());
    config.decoding_method = Some("greedy_search".into());
    match manifest.kind {
        ModelKind::OfflineTransducer => {
            config.model_config.transducer = OfflineTransducerModelConfig {
                encoder: Some(resolve_file(dir, manifest.files, "encoder")?),
                decoder: Some(resolve_file(dir, manifest.files, "decoder")?),
                joiner: Some(resolve_file(dir, manifest.files, "joiner")?),
            };
            if manifest.id.starts_with("parakeet-") {
                config.model_config.model_type = Some("nemo_transducer".into());
            }
        }
        ModelKind::Whisper => {
            config.model_config.whisper = OfflineWhisperModelConfig {
                encoder: Some(resolve_file(dir, manifest.files, "encoder")?),
                decoder: Some(resolve_file(dir, manifest.files, "decoder")?),
                language: Some(if language.is_empty() { "en" } else { language }.into()),
                task: Some("transcribe".into()),
                ..Default::default()
            };
        }
        _ => return Err("Selected model is not an offline recognizer.".into()),
    }
    let recognizer = OfflineRecognizer::create(&config)
        .ok_or_else(|| "Could not initialize the local speech recognizer.".to_string())?;
    let chunk_samples = capture.sample_rate as usize * 30;
    let mut parts = Vec::new();
    for chunk in capture.samples.chunks(chunk_samples.max(1)) {
        let stream = recognizer.create_stream();
        stream.accept_waveform(capture.sample_rate as i32, chunk);
        recognizer.decode(&stream);
        let text = stream
            .get_result()
            .ok_or_else(|| "Local speech recognizer returned no result.".to_string())?
            .text;
        if !text.trim().is_empty() {
            parts.push(text.trim().to_string());
        }
    }
    Ok(parts.join(" "))
}

fn decode_online(
    capture: &AudioCapture,
    manifest: &SpeechModelManifest,
    dir: &Path,
) -> Result<String, String> {
    let mut config = OnlineRecognizerConfig::default();
    config.feat_config.sample_rate = 16_000;
    config.feat_config.feature_dim = 80;
    config.model_config.tokens = Some(resolve_tokens(dir, manifest.files)?);
    config.model_config.num_threads = 1;
    config.model_config.provider = Some("cpu".into());
    config.model_config.modeling_unit = manifest.modeling_unit.map(str::to_string);
    config.decoding_method = Some("greedy_search".into());
    config.enable_endpoint = true;
    config.rule1_min_trailing_silence = 2.4;
    config.rule2_min_trailing_silence = 1.2;
    config.rule3_min_utterance_length = 20.0;
    match manifest.kind {
        ModelKind::OnlineTransducer => {
            config.model_config.transducer = OnlineTransducerModelConfig {
                encoder: Some(resolve_file(dir, manifest.files, "encoder")?),
                decoder: Some(resolve_file(dir, manifest.files, "decoder")?),
                joiner: Some(resolve_file(dir, manifest.files, "joiner")?),
            };
        }
        ModelKind::OnlineParaformer => {
            config.model_config.paraformer = OnlineParaformerModelConfig {
                encoder: Some(resolve_file(dir, manifest.files, "encoder")?),
                decoder: Some(resolve_file(dir, manifest.files, "decoder")?),
            };
        }
        _ => return Err("Selected model is not a streaming recognizer.".into()),
    }
    let recognizer = OnlineRecognizer::create(&config)
        .ok_or_else(|| "Could not initialize the local speech recognizer.".to_string())?;
    let stream = recognizer.create_stream();
    stream.accept_waveform(capture.sample_rate as i32, &capture.samples);
    stream.input_finished();
    while recognizer.is_ready(&stream) {
        recognizer.decode(&stream);
    }
    Ok(recognizer
        .get_result(&stream)
        .map(|result| result.text.trim().to_string())
        .unwrap_or_default())
}

pub async fn transcribe_local(
    capture: AudioCapture,
    model_id: String,
    custom_models_dir: String,
    language: String,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let manifest =
            local_model(&model_id).ok_or_else(|| format!("Unknown local model: {model_id}"))?;
        let dir = model_dir(&model_id, &custom_models_dir)
            .ok_or_else(|| format!("Unknown local model: {model_id}"))?;
        if !is_ready(&model_id, &custom_models_dir) {
            return Err(format!("Download {} before using it.", manifest.label));
        }
        match manifest.kind {
            ModelKind::OfflineTransducer | ModelKind::Whisper => {
                decode_offline(&capture, manifest, &dir, &language)
            }
            ModelKind::OnlineTransducer | ModelKind::OnlineParaformer => {
                decode_online(&capture, manifest, &dir)
            }
        }
    })
    .await
    .map_err(|error| format!("Local transcription worker failed: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_matches_orca_model_ids_and_sizes() {
        assert_eq!(LOCAL_MODELS.len(), 7);
        assert_eq!(LOCAL_MODELS[0].id, "parakeet-tdt-0.6b-v3-int8");
        assert_eq!(LOCAL_MODELS[0].size_bytes, 487_170_055);
        assert_eq!(LOCAL_MODELS[6].id, "whisper-tiny");
    }

    #[test]
    fn model_paths_only_resolve_catalog_ids() {
        assert!(model_dir("../../escape", "").is_none());
        assert!(model_dir("whisper-tiny", "").is_some());
    }
}
