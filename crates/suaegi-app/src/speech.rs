//! Voice dictation runtime: microphone capture and OpenAI transcription.
//!
//! Audio stays in memory. It is only sent over the network when the user has
//! explicitly selected one of Orca's OpenAI speech models.

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream};

const OPENAI_SPEECH_SERVICE: &str = "suaegi-openai-speech";
const OPENAI_SPEECH_ACCOUNT: &str = "default";
const MAX_SECONDS: usize = 10 * 60;
const CLOUD_SAMPLE_RATE: u32 = 16_000;

#[derive(Debug)]
pub struct AudioCapture {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

struct ActiveCapture {
    stream: Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    sample_rate: u32,
}

thread_local! {
    static ACTIVE_CAPTURE: RefCell<Option<ActiveCapture>> = const { RefCell::new(None) };
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    samples: Arc<Mutex<Vec<f32>>>,
) -> Result<Stream, String>
where
    T: Sample + SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels.max(1));
    let max_samples = config.sample_rate as usize * MAX_SECONDS;
    device
        .build_input_stream(
            *config,
            move |input: &[T], _| {
                if let Ok(mut output) = samples.try_lock() {
                    let remaining = max_samples.saturating_sub(output.len());
                    output.extend(
                        input
                            .iter()
                            .step_by(channels)
                            .take(remaining)
                            .copied()
                            .map(f32::from_sample),
                    );
                }
            },
            |_error| {},
            None,
        )
        .map_err(|error| format!("Could not open microphone input: {error}"))
}

pub fn start_capture() -> Result<(), String> {
    if ACTIVE_CAPTURE.with(|slot| slot.borrow().is_some()) {
        return Err("Voice dictation is already recording.".into());
    }
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "No microphone input device is available.".to_string())?;
    let supported = device
        .default_input_config()
        .map_err(|error| format!("Could not read microphone configuration: {error}"))?;
    let sample_rate = supported.sample_rate();
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let samples = Arc::new(Mutex::new(Vec::new()));

    let stream = match format {
        SampleFormat::I8 => build_stream::<i8>(&device, &config, samples.clone()),
        SampleFormat::I16 => build_stream::<i16>(&device, &config, samples.clone()),
        SampleFormat::I32 => build_stream::<i32>(&device, &config, samples.clone()),
        SampleFormat::I64 => build_stream::<i64>(&device, &config, samples.clone()),
        SampleFormat::U8 => build_stream::<u8>(&device, &config, samples.clone()),
        SampleFormat::U16 => build_stream::<u16>(&device, &config, samples.clone()),
        SampleFormat::U32 => build_stream::<u32>(&device, &config, samples.clone()),
        SampleFormat::U64 => build_stream::<u64>(&device, &config, samples.clone()),
        SampleFormat::F32 => build_stream::<f32>(&device, &config, samples.clone()),
        SampleFormat::F64 => build_stream::<f64>(&device, &config, samples.clone()),
        other => Err(format!("Unsupported microphone sample format: {other}")),
    }?;
    stream
        .play()
        .map_err(|error| format!("Could not start microphone input: {error}"))?;
    ACTIVE_CAPTURE.with(|slot| {
        *slot.borrow_mut() = Some(ActiveCapture {
            stream,
            samples,
            sample_rate,
        });
    });
    Ok(())
}

pub fn stop_capture() -> Result<AudioCapture, String> {
    let active = ACTIVE_CAPTURE
        .with(|slot| slot.borrow_mut().take())
        .ok_or_else(|| "Voice dictation is not recording.".to_string())?;
    drop(active.stream);
    let samples = active
        .samples
        .lock()
        .map_err(|_| "Microphone sample buffer is unavailable.".to_string())?
        .clone();
    Ok(AudioCapture {
        samples,
        sample_rate: active.sample_rate,
    })
}

pub fn cancel_capture() {
    ACTIVE_CAPTURE.with(|slot| {
        slot.borrow_mut().take();
    });
}

pub fn save_openai_key(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("OpenAI API key is required.".into());
    }
    suaegi_secrets::store(
        OPENAI_SPEECH_SERVICE,
        OPENAI_SPEECH_ACCOUNT,
        &suaegi_secrets::Secret::new(value),
    )
    .map_err(|error| error.to_string())
}

pub fn clear_openai_key() -> Result<(), String> {
    suaegi_secrets::delete(OPENAI_SPEECH_SERVICE, OPENAI_SPEECH_ACCOUNT)
        .map_err(|error| error.to_string())
}

pub fn has_openai_key() -> bool {
    suaegi_secrets::load(&suaegi_secrets::SecretRequest::new(
        OPENAI_SPEECH_SERVICE,
        OPENAI_SPEECH_ACCOUNT,
    ))
    .secret
    .is_some()
}

fn resample_to_16khz(input: &[f32], input_rate: u32) -> Vec<f32> {
    if input_rate == CLOUD_SAMPLE_RATE {
        return input.to_vec();
    }
    if input.is_empty() || input_rate == 0 {
        return Vec::new();
    }
    let output_len = ((input.len() as u64 * CLOUD_SAMPLE_RATE as u64) / input_rate as u64) as usize;
    let ratio = input_rate as f64 / CLOUD_SAMPLE_RATE as f64;
    (0..output_len)
        .map(|index| {
            let position = index as f64 * ratio;
            let left = position.floor() as usize;
            let right = (left + 1).min(input.len() - 1);
            let fraction = (position - left as f64) as f32;
            input[left] * (1.0 - fraction) + input[right] * fraction
        })
        .collect()
}

fn pcm16_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let value = if clamped < 0.0 {
            (clamped * 32768.0).round() as i16
        } else {
            (clamped * 32767.0).round() as i16
        };
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn api_model(model_id: &str) -> Option<&'static str> {
    match model_id {
        "openai-gpt-4o-mini-transcribe" => Some("gpt-4o-mini-transcribe"),
        "openai-gpt-4o-transcribe" => Some("gpt-4o-transcribe"),
        _ => None,
    }
}

fn sanitize_error(message: &str) -> String {
    let mut result = message.trim().to_string();
    if result
        .to_ascii_lowercase()
        .contains("incorrect api key provided")
    {
        return "Incorrect OpenAI API key provided.".into();
    }
    for prefix in ["sk-", "Bearer "] {
        while let Some(start) = result.find(prefix) {
            let token_start = start + prefix.len();
            let end = result[token_start..]
                .find(char::is_whitespace)
                .map_or(result.len(), |offset| token_start + offset);
            result.replace_range(start..end, "[redacted]");
        }
    }
    if result.is_empty() {
        "OpenAI transcription request failed.".into()
    } else {
        result
    }
}

pub async fn transcribe_openai(
    capture: AudioCapture,
    model_id: String,
    language: String,
) -> Result<String, String> {
    if capture.samples.is_empty() {
        return Ok(String::new());
    }
    let model = api_model(&model_id)
        .ok_or_else(|| "Select an OpenAI speech model or download a local model.".to_string())?;
    let resolved = suaegi_secrets::load(&suaegi_secrets::SecretRequest::new(
        OPENAI_SPEECH_SERVICE,
        OPENAI_SPEECH_ACCOUNT,
    ));
    let key = resolved
        .secret
        .as_ref()
        .ok_or_else(|| "Configure an OpenAI speech API key in Voice settings.".to_string())?
        .expose()
        .to_string();
    let samples = resample_to_16khz(&capture.samples, capture.sample_rate);
    let wav = pcm16_wav(&samples, CLOUD_SAMPLE_RATE);
    let mut form = reqwest::multipart::Form::new()
        .text("model", model.to_string())
        .text("response_format", "json")
        .part(
            "file",
            reqwest::multipart::Part::bytes(wav)
                .file_name("dictation.wav")
                .mime_str("audio/wav")
                .map_err(|error| error.to_string())?,
        );
    if language != "auto" && !language.is_empty() {
        form = form.text("language", language);
    }
    let response = reqwest::Client::new()
        .post("https://api.openai.com/v1/audio/transcriptions")
        .bearer_auth(key)
        .multipart(form)
        .send()
        .await
        .map_err(|error| format!("OpenAI transcription failed: {error}"))?;
    let status = response.status();
    let json: serde_json::Value = response.json().await.unwrap_or_default();
    if !status.is_success() {
        let message = json
            .pointer("/error/message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("OpenAI transcription request failed");
        return Err(format!(
            "OpenAI transcription failed: {}",
            sanitize_error(message)
        ));
    }
    json.get("text")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .map(ToOwned::to_owned)
        .ok_or_else(|| "OpenAI transcription response did not include text.".to_string())
}

pub async fn transcribe(
    capture: AudioCapture,
    model_id: String,
    models_dir: String,
    language: String,
) -> Result<String, String> {
    if api_model(&model_id).is_some() {
        transcribe_openai(capture, model_id, language).await
    } else if crate::speech_models::local_model(&model_id).is_some() {
        crate::speech_models::transcribe_local(capture, model_id, models_dir, language).await
    } else {
        Err(format!("Unknown speech model: {model_id}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_and_resampling_are_deterministic() {
        let resampled = resample_to_16khz(&[0.0, 1.0, 0.0, -1.0], 32_000);
        assert_eq!(resampled, vec![0.0, 0.0]);
        let wav = pcm16_wav(&[0.0, 1.0, -1.0], 16_000);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), 50);
    }

    #[test]
    fn cloud_errors_never_echo_api_keys() {
        let sanitized = sanitize_error("Bearer abc.secret and sk-live-secret");
        assert!(!sanitized.contains("abc.secret"));
        assert!(!sanitized.contains("sk-live-secret"));
    }
}
