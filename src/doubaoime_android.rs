use std::collections::BTreeMap;
use std::io::{ErrorKind, Read};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use opus::{Application, Channels, Encoder};
use serde::{Deserialize, Serialize};
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::pb::{self, AsrRequest};
use crate::provider::{AsrResult, Provider};

pub struct DoubaoIme;

const SAMPLE_RATE: u32 = 16000;
const FRAME_SAMPLES: usize = 960;
const FRAME_MILLIS: u64 = 60;
const VALID_FRAMES: [usize; 3] = [320, 640, FRAME_SAMPLES];
const OPUS_PACKET_BYTES: usize = 1500;
const PATIENCE: Duration = Duration::from_secs(20);
const GRACE: Duration = Duration::from_secs(5);
const DRAIN_SLICE: Duration = Duration::from_millis(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

const AID: &str = "401734";
const APP_NAME: &str = "oime";
const PACKAGE: &str = "com.bytedance.android.doubaoime";
const VERSION_CODE: &str = "100102018";
const VERSION_NAME: &str = "1.1.2";
const CHANNEL: &str = "default";
const DEVICE_MODEL: &str = "Pixel 7 Pro";
const REGISTER_URL: &str = "https://log.snssdk.com/service/2/device_register/";
const SETTINGS_URL: &str = "https://is.snssdk.com/service/settings/v3/";
const GATEWAY: &str = "wss://frontier-audio-ime-ws.doubao.com/ocean/api/v1/ws";
const USER_AGENT: &str = "com.bytedance.android.doubaoime/100102018 (Linux; U; Android 16; \
                          en_US; Pixel 7 Pro; Build/BP2A.250605.031.A2; \
                          Cronet/TTNetVersion:94cf429a 2025-11-17 QuicVersion:1f89f732 2025-05-08)";
const EMPTY_BODY: &str = "body=null";
const EMPTY_BODY_STUB: &str = "46c03b52742b3f2615a3abdf1636b754"; // md5(EMPTY_BODY)

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Device {
  cdid: String,
  openudid: String,
  clientudid: String,
  device_id: String,
  install_id: String,
  token: String,
}

impl Device {
  fn ready(&self) -> bool {
    !self.device_id.is_empty() && !self.token.is_empty()
  }
}

#[derive(Default)]
struct Transcript {
  finals: BTreeMap<u64, String>,
  partial: String,
  order: u64,
}

impl Transcript {
  fn apply(&mut self, json: &str) -> AsrResult<Option<String>> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
      return Ok(None);
    };
    let Some(results) = value.get("results").and_then(serde_json::Value::as_array) else {
      return Ok(None);
    };
    let mut interim = None;
    for result in results {
      let text = result
        .get("text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim();
      if text.is_empty() {
        continue;
      }
      if result.get("is_interim").and_then(serde_json::Value::as_bool) == Some(false) {
        let key = result
          .get("index")
          .and_then(serde_json::Value::as_u64)
          .unwrap_or(self.order);
        self.order += 1;
        self.finals.insert(key, text.to_owned());
      } else {
        self.partial = text.to_owned();
        interim = Some(self.partial.clone());
      }
    }
    Ok(interim)
  }

  fn text(&self) -> String {
    if self.finals.is_empty() {
      return self.partial.clone();
    }
    self.finals.values().cloned().collect::<Vec<String>>().join("")
  }
}

impl Provider for DoubaoIme {
  fn transcribe(&self, audio: &mut dyn Read, partial: &mut dyn FnMut(&str)) -> AsrResult<String> {
    let device = credentials()?;
    let mut session = Session::open(&device)?;
    session.stream(audio, partial)?;
    session.finish(partial)
  }
}

enum FrameState {
  First,
  Middle,
  Last,
}

struct Frame {
  bytes: Vec<u8>,
  state: FrameState,
}

struct Session {
  socket: Socket,
  device: Device,
  request_id: String,
  encoder: Encoder,
  transcript: Transcript,
  packets: u64,
  epoch: u64,
}

impl Session {
  fn open(device: &Device) -> AsrResult<Session> {
    let mut session = Session {
      socket: connect(device)?,
      device: device.clone(),
      request_id: random_uuid(),
      encoder: Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip)?,
      transcript: Transcript::default(),
      packets: 0,
      epoch: now_millis(),
    };
    session.begin()?;
    Ok(session)
  }

  /// StartTask then StartSession, waiting for each acknowledgement.
  fn begin(&mut self) -> AsrResult<()> {
    trace("start task");
    self.control("StartTask", "")?;
    expect(&mut self.socket, "TaskStarted")?;
    trace("start session");
    self.control("StartSession", &session_config(&self.device))?;
    expect(&mut self.socket, "SessionStarted")?;
    Ok(())
  }

  fn stream(&mut self, audio: &mut dyn Read, partial: &mut dyn FnMut(&str)) -> AsrResult<()> {
    let started = Instant::now();
    while let Some(samples) = read_frame(audio)? {
      let state = if self.packets == 0 {
        FrameState::First
      } else {
        FrameState::Middle
      };
      let bytes = self.encode(&samples)?;
      crate::display::level(&samples);
      self.send(&Frame { bytes, state })?;
      self.pace(started);
      self.collect(partial)?;
      if crate::stop::stopped() {
        break;
      }
    }
    if self.packets == 0 {
      return Err("no audio arrived on stdin".into());
    }
    Ok(())
  }

  fn finish(&mut self, partial: &mut dyn FnMut(&str)) -> AsrResult<String> {
    let silence = vec![0i16; FRAME_SAMPLES];
    let bytes = self.encode(&silence)?;
    self.send(&Frame {
      bytes,
      state: FrameState::Last,
    })?;
    self.control("FinishSession", "")?;
    trace("finish session");
    loop {
      let wait = self.patience();
      let response = read(&mut self.socket, wait)?;
      if let Some(message) = self.handle(&response, partial)? {
        return Ok(message);
      }
    }
  }

  /// A signal-stopped run must not sit and wait on a gateway that may be closing too.
  fn patience(&self) -> Duration {
    if crate::stop::stopped() {
      GRACE
    } else {
      PATIENCE
    }
  }

  fn encode(&mut self, samples: &[i16]) -> AsrResult<Vec<u8>> {
    if !VALID_FRAMES.contains(&samples.len()) {
      return Err(format!("{} samples is not a 20, 40 or 60 millisecond frame", samples.len()).into());
    }
    let mut packet = vec![0u8; OPUS_PACKET_BYTES];
    let written = self.encoder.encode(samples, &mut packet)?;
    packet.truncate(written);
    Ok(packet)
  }

  fn send(&mut self, frame: &Frame) -> AsrResult<()> {
    let payload = if matches!(frame.state, FrameState::Last) {
      finish_payload(self.epoch + self.packets * FRAME_MILLIS)
    } else {
      audio_payload(self.epoch + self.packets * FRAME_MILLIS)
    };
    let body = pb::encode_request(&AsrRequest {
      app_key: String::new(),
      method: "TaskRequest".into(),
      payload,
      audio: frame.bytes.clone(),
      request_id: self.request_id.clone(),
      frame_state: frame.state.code(),
    });
    self.socket.send(Message::Binary(body))?;
    self.packets += 1;
    Ok(())
  }

  fn control(&mut self, method: &str, payload: &str) -> AsrResult<()> {
    let body = pb::encode_request(&AsrRequest {
      app_key: self.device.token.clone(),
      method: method.into(),
      payload: payload.into(),
      audio: Vec::new(),
      request_id: self.request_id.clone(),
      frame_state: 0,
    });
    self.socket.send(Message::Binary(body))?;
    Ok(())
  }

  fn pace(&self, started: Instant) {
    let due = started + Duration::from_millis(self.packets * FRAME_MILLIS);
    if let Some(wait) = due.checked_duration_since(Instant::now()) {
      std::thread::sleep(wait);
    }
  }

  fn collect(&mut self, partial: &mut dyn FnMut(&str)) -> AsrResult<()> {
    loop {
      time_slice(&mut self.socket, DRAIN_SLICE);
      match self.socket.read() {
        Ok(Message::Binary(body)) => {
          let _ = self.handle(&pb::decode_response(&body), partial)?;
        }
        Ok(Message::Ping(body)) => self.socket.send(Message::Pong(body))?,
        Ok(Message::Close(_)) => return Err("the gateway closed the socket during audio".into()),
        Ok(_) => {}
        Err(tungstenite::Error::Io(error)) if error.kind() == ErrorKind::WouldBlock => return Ok(()),
        Err(error) => return Err(error.into()),
      }
    }
  }

  /// Returns the finished transcript, or a gateway failure as an error.
  fn handle(&mut self, response: &pb::AsrResponse, partial: &mut dyn FnMut(&str)) -> AsrResult<Option<String>> {
    if response.message_type.ends_with("Failed") {
      return Err(
        format!(
          "gateway {} failed: {} ({})",
          response.message_type, response.status_message, response.status_code
        )
        .into(),
      );
    }
    if let Some(text) = self.transcript.apply(&response.result_json)? {
      partial(&text);
    }
    if response.message_type == "SessionFinished" {
      return Ok(Some(self.transcript.text()));
    }
    Ok(None)
  }
}

impl FrameState {
  fn code(&self) -> u64 {
    match self {
      FrameState::First => 1,
      FrameState::Middle => 3,
      FrameState::Last => 9,
    }
  }
}

fn expect(socket: &mut Socket, wanted: &str) -> AsrResult<()> {
  loop {
    let response = read(socket, PATIENCE)?;
    if response.message_type == wanted {
      return Ok(());
    }
    if response.message_type.ends_with("Failed") {
      return Err(
        format!(
          "gateway {} failed: {} ({})",
          response.message_type, response.status_message, response.status_code
        )
        .into(),
      );
    }
  }
}

fn connect(device: &Device) -> AsrResult<Socket> {
  let url = format!("{GATEWAY}?aid={AID}&device_id={}", device.device_id);
  let mut request = url.into_client_request()?;
  let headers = request.headers_mut();
  headers.insert("User-Agent", tungstenite::http::HeaderValue::from_str(USER_AGENT)?);
  headers.insert("proto-version", tungstenite::http::HeaderValue::from_static("v2"));
  headers.insert(
    "x-custom-keepalive",
    tungstenite::http::HeaderValue::from_static("true"),
  );
  headers.insert(
    "Accept-Language",
    tungstenite::http::HeaderValue::from_static("zh-CN,zh;q=0.9"),
  );
  headers.insert("Cache-Control", tungstenite::http::HeaderValue::from_static("no-cache"));
  headers.insert("Pragma", tungstenite::http::HeaderValue::from_static("no-cache"));
  trace("opening the gateway socket");
  let (socket, _) = tungstenite::connect(request)?;
  Ok(socket)
}

fn read(socket: &mut Socket, patience: Duration) -> AsrResult<pb::AsrResponse> {
  let deadline = Instant::now() + patience;
  loop {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
      return Err(format!("the gateway said nothing for {} seconds", patience.as_secs()).into());
    }
    time_slice(socket, remaining.min(Duration::from_millis(250)));
    match socket.read() {
      Ok(Message::Binary(body)) => return Ok(pb::decode_response(&body)),
      Ok(Message::Ping(body)) => socket.send(Message::Pong(body))?,
      Ok(Message::Close(frame)) => {
        return Err(format!("the gateway closed the socket: {frame:?}").into());
      }
      Ok(_) => {}
      Err(tungstenite::Error::Io(error))
        if error.kind() == ErrorKind::WouldBlock || error.kind() == ErrorKind::Interrupted => {}
      Err(error) => return Err(error.into()),
    }
  }
}

fn time_slice(socket: &mut Socket, wait: Duration) {
  match socket.get_mut() {
    MaybeTlsStream::Plain(stream) => {
      let _ = stream.set_read_timeout(Some(wait));
    }
    MaybeTlsStream::Rustls(stream) => {
      let _ = stream.sock.set_read_timeout(Some(wait));
    }
    _ => {}
  }
}

fn read_frame(audio: &mut dyn Read) -> AsrResult<Option<Vec<i16>>> {
  let mut buffer = vec![0u8; FRAME_SAMPLES * 2];
  let mut filled = 0;
  while filled < buffer.len() {
    match audio.read(&mut buffer[filled..]) {
      Ok(0) => break,
      Ok(written) => filled += written,
      Err(error) if error.kind() == ErrorKind::Interrupted => {
        if crate::stop::stopped() {
          break;
        }
      }
      Err(error) => return Err(error.into()),
    }
  }
  if filled == 0 {
    return Ok(None);
  }
  buffer.truncate(filled);
  let padded = filled.next_multiple_of(320 * 2);
  buffer.resize(padded, 0);
  Ok(Some(samples(&buffer)))
}

fn samples(bytes: &[u8]) -> Vec<i16> {
  bytes
    .as_chunks::<2>()
    .0
    .iter()
    .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
    .collect()
}

fn audio_payload(timestamp: u64) -> String {
  format!(r#"{{"extra":{{}},"timestamp_ms":{timestamp}}}"#)
}

fn finish_payload(timestamp: u64) -> String {
  format!(r#"{{"extra":{{"finish_audio":true,"force_asr_twopass":true}},"timestamp_ms":{timestamp}}}"#)
}

fn session_config(device: &Device) -> String {
  serde_json::json!({
    "audio_info": { "channel": 1, "format": "speech_opus", "sample_rate": 16000 },
    "enable_punctuation": true,
    "enable_speech_rejection": false,
    "extra": {
      "app_name": "com.android.chrome",
      "app_version": VERSION_NAME,
      "aid": AID,
      "cell_compress_rate": 8,
      "did": device.device_id,
      "enable_asr_threepass": true,
      "enable_asr_twopass": true,
      "enable_print_chinese": false,
      "disable_user_words": true,
      "enable_text_post_process": true,
      "asr_text_post_process_type": "last_post_process",
      "input_mode": "tool",
      "strong_ddc": true,
      "use_twopass_retry": true,
      "update_version_code": VERSION_CODE,
      "version_code": VERSION_CODE,
      "version_name": VERSION_NAME
    }
  })
  .to_string()
}

fn credentials() -> AsrResult<Device> {
  let path = state_path()?;
  if let Ok(text) = std::fs::read_to_string(&path) {
    if let Ok(device) = serde_json::from_str::<Device>(&text) {
      if device.ready() {
        return Ok(device);
      }
    }
  }
  let mut device = identity();
  trace(&format!("registering a device at {REGISTER_URL}"));
  register(&mut device)?;
  trace(&format!("fetching a recognition token from {SETTINGS_URL}"));
  issue_token(&mut device)?;
  store(&path, &device)?;
  trace(&format!("doubao device {}", device.device_id));
  Ok(device)
}

fn register(device: &mut Device) -> AsrResult<()> {
  let url = format!("{REGISTER_URL}?{}", common_query(device, false));
  let body = serde_json::json!({
    "magic_tag": "ss_app_log",
    "header": register_header(device),
    "_gen_time": now_millis()
  });
  let text = post_json(&url, &body.to_string())?;
  let value: serde_json::Value = serde_json::from_str(&text)?;
  let device_id = numeric(&value, "device_id").unwrap_or_default();
  if device_id == "0" || device_id.is_empty() {
    return Err(format!("doubao refused to register a device: {text}").into());
  }
  device.device_id = device_id;
  device.install_id = numeric(&value, "install_id").unwrap_or_default();
  Ok(())
}

fn issue_token(device: &mut Device) -> AsrResult<()> {
  let url = format!("{SETTINGS_URL}?{}", common_query(device, true));
  let text = post_json_with_stub(&url, EMPTY_BODY)?;
  let value: serde_json::Value = serde_json::from_str(&text)?;
  value
    .pointer("/data/settings/asr_config/app_key")
    .and_then(serde_json::Value::as_str)
    .map(str::to_owned)
    .filter(|token| !token.is_empty())
    .map(|token| device.token = token)
    .ok_or_else(|| format!("doubao gave no recognition token: {text}").into())
}

fn register_header(device: &Device) -> serde_json::Value {
  serde_json::json!({
    "device_id": 0,
    "install_id": 0,
    "aid": AID,
    "app_name": APP_NAME,
    "version_code": VERSION_CODE,
    "version_name": VERSION_NAME,
    "manifest_version_code": VERSION_CODE,
    "update_version_code": VERSION_CODE,
    "channel": CHANNEL,
    "package": PACKAGE,
    "device_platform": "android",
    "os": "android",
    "os_api": "34",
    "os_version": "16",
    "device_type": DEVICE_MODEL,
    "device_brand": "google",
    "device_model": DEVICE_MODEL,
    "resolution": "1080*2400",
    "dpi": "420",
    "language": "zh",
    "timezone": 8,
    "access": "wifi",
    "rom": "UP1A.231005.007",
    "rom_version": "UP1A.231005.007",
    "openudid": device.openudid,
    "clientudid": device.clientudid,
    "cdid": device.cdid,
    "region": "CN",
    "tz_name": "Asia/Shanghai",
    "tz_offset": 28800,
    "sim_region": "cn",
    "carrier_region": "cn",
    "cpu_abi": "arm64-v8a",
    "build_serial": "unknown",
    "not_request_sender": 0,
    "sig_hash": "",
    "google_aid": "",
    "mc": "",
    "serial_number": ""
  })
}

fn common_query(device: &Device, with_device_id: bool) -> String {
  let mut pairs: Vec<(&str, String)> = vec![
    ("device_platform", "android".into()),
    ("os", "android".into()),
    ("ssmix", "a".into()),
    ("_rticket", now_millis().to_string()),
    ("cdid", device.cdid.clone()),
    ("channel", CHANNEL.into()),
    ("aid", AID.into()),
    ("app_name", APP_NAME.into()),
    ("version_code", VERSION_CODE.into()),
    ("version_name", VERSION_NAME.into()),
  ];
  // shortcut: every value here is a compile-time constant or a UUID, so it needs no escaping.
  if with_device_id {
    pairs.push(("device_id", device.device_id.clone()));
  } else {
    pairs.extend([
      ("manifest_version_code", VERSION_CODE.into()),
      ("update_version_code", VERSION_CODE.into()),
      ("resolution", "1080*2400".into()),
      ("dpi", "420".into()),
      ("device_type", DEVICE_MODEL.into()),
      ("device_brand", "google".into()),
      ("language", "zh".into()),
      ("os_api", "34".into()),
      ("os_version", "16".into()),
      ("ac", "wifi".into()),
    ]);
  }
  pairs
    .iter()
    .map(|(key, value)| format!("{key}={value}"))
    .collect::<Vec<String>>()
    .join("&")
}

fn post_json(url: &str, body: &str) -> AsrResult<String> {
  post(url, body, &[("Content-Type", "application/json")])
}

fn post_json_with_stub(url: &str, body: &str) -> AsrResult<String> {
  post(url, body, &[("x-ss-stub", EMPTY_BODY_STUB)])
}

/// ByteDance endpoints answer 504 through this network now and then, so try twice.
fn post(url: &str, body: &str, headers: &[(&str, &str)]) -> AsrResult<String> {
  let mut failure = String::from("no attempt was made");
  for _ in 0..REQUEST_ATTEMPTS {
    let mut request = agent().post(url);
    for (name, value) in headers {
      request = request.set(name, value);
    }
    match request.send_string(body) {
      Ok(response) => return Ok(response.into_string()?),
      Err(ureq::Error::Status(code, response)) => failure = format!("HTTP {code}: {}", response.into_string()?),
      Err(error) => failure = error.to_string(),
    }
    std::thread::sleep(RETRY_PAUSE);
  }
  Err(failure.into())
}

fn agent() -> ureq::Agent {
  ureq::AgentBuilder::new()
    .user_agent(USER_AGENT)
    .timeout_connect(CONNECT_TIMEOUT)
    .timeout_read(READ_TIMEOUT)
    .timeout_write(WRITE_TIMEOUT)
    .build()
}

const REQUEST_ATTEMPTS: usize = 2;
const RETRY_PAUSE: Duration = Duration::from_secs(2);

fn trace(message: &str) {
  crate::display::status(message);
}

fn numeric(value: &serde_json::Value, key: &str) -> Option<String> {
  match value.get(key) {
    Some(serde_json::Value::String(text)) => Some(text.clone()),
    Some(serde_json::Value::Number(number)) => Some(number.to_string()),
    _ => None,
  }
}

fn state_path() -> AsrResult<PathBuf> {
  let root = match std::env::var_os("XDG_STATE_HOME") {
    Some(path) => PathBuf::from(path),
    None => PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?).join(".local/state"),
  };
  Ok(root.join("ji").join("device.json"))
}

fn store(path: &Path, device: &Device) -> AsrResult<()> {
  if let Some(directory) = path.parent() {
    std::fs::create_dir_all(directory)?;
  }
  std::fs::write(path, serde_json::to_string_pretty(device)?)?;
  // The file holds the recognition token, so keep it off the rest of the account.
  std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
  Ok(())
}

fn identity() -> Device {
  Device {
    cdid: random_uuid(),
    openudid: random_hex(8),
    clientudid: random_uuid(),
    ..Device::default()
  }
}

fn random_uuid() -> String {
  let text = random_hex(16);
  format!(
    "{}-{}-{}-{}-{}",
    &text[0..8],
    &text[8..12],
    &text[12..16],
    &text[16..20],
    &text[20..32]
  )
}

fn random_hex(count: usize) -> String {
  let mut bytes = vec![0u8; count];
  std::fs::File::open("/dev/urandom")
    .and_then(|mut noise| noise.read_exact(&mut bytes))
    .expect("cannot read /dev/urandom for entropy");
  bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn now_millis() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|since| since.as_millis() as u64)
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn finals_win_and_interims_collapse_into_the_partial() {
    let mut transcript = Transcript::default();
    let interim = transcript
      .apply(r#"{"results":[{"text":"你好","is_interim":true}]}"#)
      .unwrap();
    assert_eq!(interim.as_deref(), Some("你好"));
    assert_eq!(transcript.text(), "你好");
    transcript
      .apply(r#"{"results":[{"text":"你好，世界。","index":0,"is_interim":false}]}"#)
      .unwrap();
    assert_eq!(transcript.text(), "你好，世界。");
  }

  #[test]
  fn resultless_payloads_are_heartbeats_not_failures() {
    let mut transcript = Transcript::default();
    assert_eq!(transcript.apply("not json").unwrap(), None);
    assert_eq!(transcript.apply(r#"{"payload":{}}"#).unwrap(), None);
  }

  #[test]
  fn stored_credentials_are_not_readable_by_the_rest_of_the_account() {
    let directory = std::env::temp_dir().join(format!("ji-store-{}", std::process::id()));
    let path = directory.join("device.json");
    let mut device = identity();
    device.device_id = "42".into();
    device.token = "secret".into();
    store(&path, &device).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    std::fs::remove_dir_all(&directory).unwrap();
    assert_eq!(mode & 0o077, 0);
  }

  #[test]
  fn entropy_comes_from_a_single_read_of_urandom() {
    let first = random_hex(8);
    assert_eq!(first.len(), 16);
    assert_ne!(first, random_hex(8));
  }

  #[test]
  fn frame_state_matches_the_client_order() {
    assert_eq!(FrameState::First.code(), 1);
    assert_eq!(FrameState::Middle.code(), 3);
    assert_eq!(FrameState::Last.code(), 9);
  }

  #[test]
  fn audio_and_finish_payloads_differ_only_in_the_extra_flags() {
    assert_eq!(audio_payload(120), r#"{"extra":{},"timestamp_ms":120}"#);
    assert_eq!(
      finish_payload(120),
      r#"{"extra":{"finish_audio":true,"force_asr_twopass":true},"timestamp_ms":120}"#
    );
  }

  #[test]
  fn a_partial_read_pads_up_to_the_next_valid_frame() {
    let mut audio = &[0u8; 700][..];
    let frame = read_frame(&mut audio).unwrap().unwrap();
    assert_eq!(frame.len(), 640);
  }

  #[test]
  fn a_closed_stream_ends_the_session_without_a_frame() {
    assert!(read_frame(&mut std::io::empty()).unwrap().is_none());
  }
}
