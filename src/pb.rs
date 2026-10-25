// Hand-rolled protobuf codec for the Doubao IME ASR gateway.
// There is no .proto anywhere; the field layout is what the IME client sends on the wire.

pub struct AsrRequest {
  pub app_key: String,
  pub method: String,
  pub payload: String,
  pub audio: Vec<u8>,
  pub request_id: String,
  pub frame_state: u64,
}

pub struct AsrResponse {
  pub message_type: String,
  pub status_code: i64,
  pub status_message: String,
  pub result_json: String,
}

pub fn encode_request(request: &AsrRequest) -> Vec<u8> {
  let mut body = Vec::new();
  if !request.app_key.is_empty() {
    append_string(&mut body, FIELD_APP_KEY, &request.app_key);
  }
  append_string(&mut body, FIELD_BUNDLE, "ASR");
  append_string(&mut body, FIELD_METHOD, &request.method);
  if !request.payload.is_empty() {
    append_string(&mut body, FIELD_PAYLOAD, &request.payload);
  }
  if !request.audio.is_empty() {
    append_bytes(&mut body, FIELD_AUDIO, &request.audio);
  }
  append_string(&mut body, FIELD_REQUEST_ID, &request.request_id);
  if request.frame_state != 0 {
    append_uint_field(&mut body, FIELD_FRAME_STATE, request.frame_state);
  }
  body
}

pub fn decode_response(body: &[u8]) -> AsrResponse {
  let mut response = AsrResponse {
    message_type: String::new(),
    status_code: 0,
    status_message: String::new(),
    result_json: String::new(),
  };
  let mut cursor = 0;
  while cursor < body.len() {
    let Some(key) = read_uint(body, &mut cursor) else {
      break;
    };
    let field = key >> 3;
    let wire = key & 0x7;
    match (field, wire) {
      (FIELD_MESSAGE_TYPE, WIRE_BYTES) => response.message_type = read_string(body, &mut cursor),
      (FIELD_STATUS_CODE, WIRE_UINT) => response.status_code = read_uint(body, &mut cursor).unwrap_or_default() as i64,
      (FIELD_STATUS_MESSAGE, WIRE_BYTES) => response.status_message = read_string(body, &mut cursor),
      (FIELD_RESULT, WIRE_BYTES) => response.result_json = read_string(body, &mut cursor),
      _ => skip_field(body, &mut cursor, wire),
    }
  }
  response
}

const FIELD_APP_KEY: u64 = 2;
const FIELD_BUNDLE: u64 = 3;
const FIELD_METHOD: u64 = 5;
const FIELD_PAYLOAD: u64 = 6;
const FIELD_AUDIO: u64 = 7;
const FIELD_REQUEST_ID: u64 = 8;
const FIELD_FRAME_STATE: u64 = 9;
const FIELD_MESSAGE_TYPE: u64 = 4;
const FIELD_STATUS_CODE: u64 = 5;
const FIELD_STATUS_MESSAGE: u64 = 6;
const FIELD_RESULT: u64 = 7;
const WIRE_UINT: u64 = 0;
const WIRE_BYTES: u64 = 2;

fn append_uint(body: &mut Vec<u8>, mut value: u64) {
  loop {
    let byte = (value & 0x7f) as u8;
    value >>= 7;
    if value == 0 {
      body.push(byte);
      return;
    }
    body.push(byte | 0x80);
  }
}

fn append_uint_field(body: &mut Vec<u8>, field: u64, value: u64) {
  append_key(body, field, WIRE_UINT);
  append_uint(body, value);
}

fn append_key(body: &mut Vec<u8>, field: u64, wire: u64) {
  append_uint(body, (field << 3) | wire);
}

fn append_bytes(body: &mut Vec<u8>, field: u64, value: &[u8]) {
  append_key(body, field, WIRE_BYTES);
  append_uint(body, value.len() as u64);
  body.extend_from_slice(value);
}

fn append_string(body: &mut Vec<u8>, field: u64, value: &str) {
  append_bytes(body, field, value.as_bytes());
}

fn read_uint(body: &[u8], cursor: &mut usize) -> Option<u64> {
  let mut value = 0u64;
  let mut shift = 0;
  while *cursor < body.len() {
    let byte = body[*cursor];
    *cursor += 1;
    value |= ((byte & 0x7f) as u64) << shift;
    if byte & 0x80 == 0 {
      return Some(value);
    }
    shift += 7;
  }
  None
}

fn read_string(body: &[u8], cursor: &mut usize) -> String {
  let Some(length) = read_uint(body, cursor) else {
    return String::new();
  };
  let end = (*cursor + length as usize).min(body.len());
  let text = String::from_utf8_lossy(&body[*cursor..end]).into_owned();
  *cursor = end;
  text
}

fn skip_field(body: &[u8], cursor: &mut usize, wire: u64) {
  match wire {
    WIRE_UINT => {
      read_uint(body, cursor);
    }
    2 => {
      read_string(body, cursor);
    }
    _ => *cursor = body.len(),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn request(audio: &[u8], frame_state: u64) -> AsrRequest {
    AsrRequest {
      app_key: "key".into(),
      method: "TaskRequest".into(),
      payload: "{}".into(),
      audio: audio.to_vec(),
      request_id: "req".into(),
      frame_state,
    }
  }

  #[test]
  fn encodes_frames_in_the_client_field_order() {
    let body = encode_request(&request(&[1u8, 2u8], 9));
    let expected: Vec<u8> = vec![
      0x12, 0x03, b'k', b'e', b'y', // 2 app_key
      0x1a, 0x03, b'A', b'S', b'R', // 3 bundle
      0x2a, 0x0b, b'T', b'a', b's', b'k', b'R', b'e', b'q', b'u', b'e', b's', b't', // 5 method
      0x32, 0x02, b'{', b'}', // 6 payload
      0x3a, 0x02, 1, 2, // 7 audio
      0x42, 0x03, b'r', b'e', b'q', // 8 request_id
      0x48, 9, // 9 frame_state
    ];
    assert_eq!(body, expected);
  }

  #[test]
  fn omits_empty_optionals_and_zero_frame_state() {
    let mut quiet = request(&[], 0);
    quiet.app_key = String::new();
    quiet.payload = String::new();
    let body = encode_request(&quiet);
    assert!(!body.contains(&0x48));
    assert!(!body.contains(&0x3a));
  }

  #[test]
  fn decodes_a_final_result_and_ignores_unknown_fields() {
    let mut body = vec![0x5a, 0x01, 0xff]; // field 11, unknown bytes
    append_string(&mut body, FIELD_MESSAGE_TYPE, "SessionFinished");
    append_uint_field(&mut body, FIELD_STATUS_CODE, 0);
    append_string(&mut body, FIELD_RESULT, r#"{"results":[]}"#);
    let response = decode_response(&body);
    assert_eq!(response.message_type, "SessionFinished");
    assert_eq!(response.status_code, 0);
    assert_eq!(response.result_json, r#"{"results":[]}"#);
  }
}
