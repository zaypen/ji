use std::io::{ErrorKind, Read};

use crate::provider::{AsrResult, Provider};

pub struct Audiocpp {
  pub base: String,
  pub model: String,
}

const BOUNDARY: &str = "ji-boundary";

impl Provider for Audiocpp {
  fn transcribe(&self, audio: &mut dyn Read, partial: &mut dyn FnMut(&str)) -> AsrResult<String> {
    if self.streaming() {
      return self.live(audio, partial);
    }
    let pcm = crate::stop::read_captured(audio)?;
    if pcm.is_empty() {
      return Err("no audio reached the process".into());
    }
    // One upload, one answer. The meter has nothing live left to follow because the whole
    // recording is already here, so it reports how loud everything that arrived was.
    crate::display::status(&format!("{} bytes captured", pcm.len()));
    crate::display::level_from_bytes(&pcm);
    self.upload(&wave(&pcm), partial)
  }
}

impl Audiocpp {
  /// A model whose name ends in `-stream` is the server's own spelling for "this one is
  /// configured for it", so the name is the switch and there is no flag to learn.
  fn streaming(&self) -> bool {
    self.model.ends_with("-stream")
  }

  /// The route whose request body *is* the recording: headerless PCM, sent as it arrives,
  /// transcript coming back on the same connection. No WAV container and no multipart body,
  /// so there is nothing here assembled wrong. The parameters are a promise: a stream
  /// without a header cannot be checked, so declaring 48 kHz as 16 kHz produces a confident
  /// wrong transcript instead of an error. This one declares what `ji` always accepts.
  fn live(&self, audio: &mut dyn Read, partial: &mut dyn FnMut(&str)) -> AsrResult<String> {
    let url = format!(
      "{}/v1/audio/transcriptions/live?model={}&sample_rate={SAMPLE_RATE}&channels=1&sample_format=s16le",
      self.base.trim_end_matches('/'),
      self.model
    );
    let mut recording = Metered { audio };
    // Close the connection when the answer is finished. Kept alive, the server's chunked
    // answer stream confuses ureq's chunk decoder on the first read; a stream we already
    // know how to end is worth the header.
    let response = ureq::post(&url)
      .set("Accept", "text/event-stream")
      .set("Connection", "close")
      .send(&mut recording)
      .map_err(|error| failed(&url, error))?;
    stream(response.into_reader(), partial)
  }

  fn upload(&self, wav: &[u8], partial: &mut dyn FnMut(&str)) -> AsrResult<String> {
    let url = format!("{}/v1/audio/transcriptions", self.base.trim_end_matches('/'));
    let body = self.multipart(wav);
    let response = ureq::post(&url)
      .set("Content-Type", &format!("multipart/form-data; boundary={BOUNDARY}"))
      .send_bytes(&body)
      .map_err(|error| failed(&url, error))?;
    if response
      .header("content-type")
      .unwrap_or_default()
      .contains("text/event-stream")
    {
      stream(response.into_reader(), partial)
    } else {
      json(response.into_string()?)
    }
  }

  fn multipart(&self, wav: &[u8]) -> Vec<u8> {
    let mut body = format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n").into_bytes();
    body.extend_from_slice(wav);
    body.extend_from_slice(b"\r\n");
    field(&mut body, "model", &self.model);
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
  }
}

/// The body of a live request: the recording itself, watched for level as it goes out. When
/// a signal says the dictation is over this reports the end, because the upload has to
/// close with a proper last chunk instead of a dropped connection — otherwise a transcript
/// cut short would look the same as a speaker who stopped.
struct Metered<'a> {
  audio: &'a mut dyn Read,
}

impl Read for Metered<'_> {
  fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
    if crate::stop::stopped() {
      return Ok(0);
    }
    let got = self.audio.read(buffer)?;
    if got > 0 {
      crate::display::level_from_bytes(&buffer[..got]);
    }
    Ok(got)
  }
}

/// A rejected request, with whatever the server said attached.
fn failed(url: &str, error: ureq::Error) -> Box<dyn std::error::Error> {
  match error {
    ureq::Error::Status(code, rejected) => {
      format!("{url} answered {code}: {}", rejected.into_string().unwrap_or_default()).into()
    }
    other => other.into(),
  }
}

/// One SSE line worth acting on. The server documents four: a piece to continue with, a
/// provisional answer to replace what is shown, the finished answer, or a complaint.
enum Piece {
  Delta(String),
  Replace(String),
  Done(String),
  Fault(String),
}

fn piece(payload: &str) -> Option<Piece> {
  let value: serde_json::Value = serde_json::from_str(payload).ok()?;
  if let Some(error) = value.get("error") {
    return Some(Piece::Fault(error.to_string()));
  }
  let text = || value.get("text").and_then(serde_json::Value::as_str).map(str::to_owned);
  match value.get("type").and_then(serde_json::Value::as_str) {
    Some("transcript.text.delta") => value
      .get("delta")
      .and_then(serde_json::Value::as_str)
      .map(|piece| Piece::Delta(piece.to_owned())),
    Some("transcript.text.partial") => text().map(Piece::Replace),
    Some("transcript.text.done") => text().map(Piece::Done),
    // A server that does not name its events still means it by the field it fills.
    _ => value
      .get("delta")
      .and_then(serde_json::Value::as_str)
      .map(|piece| Piece::Delta(piece.to_owned()))
      .or_else(|| text().map(Piece::Done)),
  }
}

/// The events break the answer into pieces that *continue* each other, which is the
/// opposite of the way the IME gateway restates the whole utterance every time. Either way
/// the caller sees a complete view, because the running text is assembled here.
fn stream(mut reader: impl Read, partial: &mut dyn FnMut(&str)) -> AsrResult<String> {
  let mut committed = String::new();
  let mut provisional = String::new();
  let mut done = String::new();
  let mut pending: Vec<u8> = Vec::new();
  let mut window = [0u8; 8192];
  loop {
    match reader.read(&mut window) {
      Ok(0) => break,
      Ok(got) => pending.extend_from_slice(&window[..got]),
      Err(error) if error.kind() == ErrorKind::Interrupted => continue,
      Err(error) => return Err(error.into()),
    }
    while let Some(cut) = pending.iter().position(|byte| *byte == b'\n') {
      let bytes: Vec<u8> = pending.drain(..=cut).collect();
      let line = String::from_utf8_lossy(&bytes).trim().to_owned();
      let Some(payload) = line.strip_prefix("data:") else {
        continue;
      };
      let payload = payload.trim();
      if payload == "[DONE]" {
        return Ok(finalise(done, committed, provisional));
      }
      match piece(payload) {
        Some(Piece::Delta(delta)) => {
          committed.push_str(&delta);
          provisional.clear();
          partial(&committed);
        }
        // A revising model carries the still-open part separately, and the documentation
        // is blunt: replace it, do not append it to what is already committed.
        Some(Piece::Replace(text)) => {
          provisional = text;
          let view = format!("{committed}{provisional}");
          partial(&view);
        }
        Some(Piece::Done(text)) => done = text,
        Some(Piece::Fault(fault)) => return Err(format!("the audiocpp provider said: {fault}").into()),
        None => {}
      }
    }
  }
  Ok(finalise(done, committed, provisional))
}

/// The `done` event carries the punctuated answer, so it wins over whatever was assembled
/// from the pieces when both are present.
fn finalise(done: String, committed: String, provisional: String) -> String {
  if done.is_empty() {
    format!("{committed}{provisional}")
  } else {
    done
  }
}

fn json(body: String) -> AsrResult<String> {
  let value: serde_json::Value = serde_json::from_str(&body)?;
  if let Some(error) = value.get("error") {
    return Err(format!("the audiocpp provider said: {}", error).into());
  }
  value
    .get("text")
    .and_then(serde_json::Value::as_str)
    .map(str::to_owned)
    .ok_or_else(|| format!("unexpected transcription response: {body}").into())
}

const SAMPLE_RATE: u32 = 16000;

/// A part carrying a short text value: delimiter, header, blank line, value, line break.
fn field(body: &mut Vec<u8>, name: &str, value: &str) {
  body.extend_from_slice(
    format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes(),
  );
}
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
  fn multipart_closes_each_part_before_opening_the_next() {
    let body = Audiocpp {
      base: "http://x".into(),
      model: "qwen3-asr".into(),
    }
    .multipart(b"RIFF");
    let text = String::from_utf8_lossy(&body);
    // The audio part must be closed before the model part is opened, or the server reads no
    // model field at all and the whole request comes back as a failure. A check that only
    // looks for the field name is blind to exactly this.
    assert!(text.starts_with("--ji-boundary\r\nContent-Disposition: form-data; name=\"file\""));
    assert!(text.contains(
      "RIFF\r\n--ji-boundary\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nqwen3-asr\r\n--ji-boundary--\r\n"
    ));
    // The upload path never asks for a stream; a streaming model goes live instead.
    assert!(!text.contains("name=\"stream\""));
  }

  #[test]
  fn a_streaming_model_takes_the_live_route() {
    let on_disk = Audiocpp {
      base: "http://x".into(),
      model: "qwen3-asr".into(),
    };
    let live = Audiocpp {
      base: "http://x".into(),
      model: "sense-asr-stream".into(),
    };
    assert!(!on_disk.streaming());
    assert!(live.streaming());
  }

  /// The whole reason this exists: pieces continue each other, and whatever is shown is
  /// the whole answer so far, not the newest fragment.
  #[test]
  fn pieces_accumulate_into_the_whole_view() {
    let wire = "data: {\"type\":\"transcript.text.delta\",\"delta\":\"one\"}\n\n\
                data: {\"type\":\"transcript.text.delta\",\"delta\":\" two\"}\n\n\
                data: {\"type\":\"transcript.text.done\",\"text\":\"one two.\"}\n\n\
                data: [DONE]\n\n";
    let mut shown = Vec::new();
    let answer = stream(std::io::Cursor::new(wire), &mut |text: &str| {
      shown.push(text.to_owned())
    })
    .unwrap();
    assert_eq!(shown, ["one", "one two"]);
    assert_eq!(answer, "one two.");
  }

  #[test]
  fn a_provisional_answer_replaces_the_open_part_instead_of_sewing_it_in_twice() {
    let wire = "data: {\"type\":\"transcript.text.delta\",\"delta\":\"one\"}\n\n\
                data: {\"type\":\"transcript.text.partial\",\"text\":\"next\"}\n\n\
                data: {\"type\":\"transcript.text.partial\",\"text\":\"next part\"}\n\n\
                data: {\"type\":\"transcript.text.done\",\"text\":\"one two.\"}\n\n";
    let mut shown = Vec::new();
    let answer = stream(std::io::Cursor::new(wire), &mut |text: &str| {
      shown.push(text.to_owned())
    })
    .unwrap();
    assert_eq!(shown, ["one", "onenext", "onenext part"]);
    assert_eq!(answer, "one two.");
  }

  #[test]
  fn a_stream_that_stops_before_done_keeps_what_arrived() {
    let wire = "data: {\"type\":\"transcript.text.delta\",\"delta\":\"half\"}\n\n";
    let mut shown = Vec::new();
    let answer = stream(std::io::Cursor::new(wire), &mut |text: &str| {
      shown.push(text.to_owned())
    })
    .unwrap();
    assert_eq!(answer, "half");
  }

  #[test]
  fn a_complaint_on_the_stream_is_an_error_and_not_an_empty_answer() {
    let wire = "data: {\"error\":{\"message\":\"bad model\"}}\n\ndata: [DONE]\n\n";
    let failure = stream(std::io::Cursor::new(wire), &mut |_| ());
    assert!(failure.unwrap_err().to_string().contains("bad model"));
  }
}
