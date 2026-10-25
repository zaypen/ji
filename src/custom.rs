use std::io::Read;

use crate::provider::{AsrResult, Provider};

pub struct Custom {
  pub base: String,
  pub model: String,
}

const BOUNDARY: &str = "ji-boundary";

impl Provider for Custom {
  fn transcribe(&self, audio: &mut dyn Read, _partial: &mut dyn FnMut(&str)) -> AsrResult<String> {
    let pcm = crate::stop::read_captured(audio)?;
    if pcm.is_empty() {
      return Err("no audio reached the process".into());
    }
    // Batch answers once, so there is no current text to show and no per-frame meter to
    // chase. Loudness of everything that arrived is the evidence worth reporting instead.
    crate::display::status(&format!("{} bytes captured", pcm.len()));
    crate::display::level_from_bytes(&pcm);
    self.recognise(&wave(&pcm))
  }
}

impl Custom {
  fn recognise(&self, wav: &[u8]) -> AsrResult<String> {
    let url = format!("{}/v1/audio/transcriptions", self.base.trim_end_matches('/'));
    let body = self.multipart(wav);
    let response = ureq::post(&url)
      .set("Content-Type", &format!("multipart/form-data; boundary={BOUNDARY}"))
      .send_bytes(&body)?
      .into_string()?;
    let value: serde_json::Value = serde_json::from_str(&response)?;
    value
      .get("text")
      .and_then(serde_json::Value::as_str)
      .map(str::to_owned)
      .ok_or_else(|| format!("unexpected transcription response: {response}").into())
  }

  fn multipart(&self, wav: &[u8]) -> Vec<u8> {
    let part = format!(
      "--{BOUNDARY}\r\n\
       Content-Disposition: form-data; name=\"model\"\r\n\r\n{}\r\n\
       --{BOUNDARY}--\r\n",
      self.model
    );
    let mut body = format!(
      "--{BOUNDARY}\r\n\
       Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n\
       Content-Type: audio/wav\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(wav);
    body.extend_from_slice(part.as_bytes());
    body
  }
}

const SAMPLE_RATE: u32 = 16000;
const BITS_PER_SAMPLE: u16 = 16;

pub fn wave(pcm: &[u8]) -> Vec<u8> {
  let mut wav = Vec::with_capacity(pcm.len() + 44);
  wav.extend_from_slice(b"RIFF");
  push_u32(&mut wav, 36 + pcm.len() as u32);
  wav.extend_from_slice(b"WAVEfmt ");
  push_u32(&mut wav, 16);
  push_u16(&mut wav, 1);
  push_u16(&mut wav, 1);
  push_u32(&mut wav, SAMPLE_RATE);
  push_u32(&mut wav, SAMPLE_RATE * (BITS_PER_SAMPLE / 8) as u32);
  push_u16(&mut wav, BITS_PER_SAMPLE / 8);
  push_u16(&mut wav, BITS_PER_SAMPLE);
  wav.extend_from_slice(b"data");
  push_u32(&mut wav, pcm.len() as u32);
  wav.extend_from_slice(pcm);
  wav
}

fn push_u32(wav: &mut Vec<u8>, value: u32) {
  wav.extend_from_slice(&value.to_le_bytes());
}

fn push_u16(wav: &mut Vec<u8>, value: u16) {
  wav.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn wave_describes_sixteen_kilohertz_mono_sixteen_bit_pcm() {
    let wav = wave(&[0u8; 3200]);
    assert_eq!(wav.len(), 3244);
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(u32_from(&wav, 4), 3236);
    assert_eq!(&wav[8..16], b"WAVEfmt ");
    assert_eq!(u16_from(&wav, 20), 1);
    assert_eq!(u16_from(&wav, 22), 1);
    assert_eq!(u32_from(&wav, 24), 16000);
    assert_eq!(u32_from(&wav, 28), 32000);
    assert_eq!(u16_from(&wav, 32), 2);
    assert_eq!(u16_from(&wav, 34), 16);
    assert_eq!(&wav[36..44], b"data\x80\x0c\0\0");
  }

  fn u32_from(wav: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(wav[offset..offset + 4].try_into().unwrap())
  }

  fn u16_from(wav: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(wav[offset..offset + 2].try_into().unwrap())
  }

  #[test]
  fn multipart_carries_both_form_fields() {
    let body = Custom {
      base: "http://x".into(),
      model: "qwen3-asr".into(),
    }
    .multipart(b"RIFF");
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("name=\"file\""));
    assert!(text.contains("name=\"model\""));
    assert!(text.contains("qwen3-asr"));
  }
}
