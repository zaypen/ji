# Doubao IME ASR gateway

This is the reverse-engineered route used by `ji --provider doubaoime_android`. It is not an
official API. Field numbers and payloads come from observed Android client behaviour,
as documented by [Douvo](https://github.com/rhinoc/douvo). Expect changes without
notice.

## Endpoint

```
wss://frontier-audio-ime-ws.doubao.com/ocean/api/v1/ws?aid=401734&device_id=<id>
```

Request headers:

| Header | Value |
| --- | --- |
| `User-Agent` | the IME client user agent, ending in `... Cronet/TTNetVersion:94cf429a 2025-11-17 QuicVersion:1f89f732 2025-05-08)` |
| `proto-version` | `v2` |
| `x-custom-keepalive` | `true` |
| `Accept-Language` | `zh-CN,zh;q=0.9` |
| `Cache-Control` / `Pragma` | `no-cache` |

The gateway accepts only `aid` and `device_id` in the query. The expanded mobile query
is rejected by the front end, so do not add to it.

## Getting an identity

There is no account and no key. The client registers a handset and receives a token.

```
POST https://log.snssdk.com/service/2/device_register/?<common query, no device_id>
Content-Type: application/json
```

```json
{"magic_tag": "ss_app_log", "header": {
  "device_id": 0, "install_id": 0, "aid": 401734, "app_name": "oime",
  "version_code": 100102018, "version_name": "1.1.2",
  "manifest_version_code": 100102018, "update_version_code": 100102018,
  "channel": "default", "package": "com.bytedance.android.doubaoime",
  "device_platform": "android", "os": "android", "os_api": "34", "os_version": "16",
  "device_type": "Pixel 7 Pro", "device_brand": "google", "device_model": "Pixel 7 Pro",
  "resolution": "1080*2400", "dpi": "420", "language": "zh", "timezone": 8,
  "access": "wifi", "rom": "UP1A.231005.007", "rom_version": "UP1A.231005.007",
  "openudid": "<8 random bytes hex>", "clientudid": "<uuid>", "cdid": "<uuid>",
  "region": "CN", "tz_name": "Asia/Shanghai", "tz_offset": 28800,
  "sim_region": "cn", "carrier_region": "cn", "cpu_abi": "arm64-v8a",
  "build_serial": "unknown", "not_request_sender": 0,
  "sig_hash": "", "google_aid": "", "mc": "", "serial_number": ""
}, "_gen_time": <epoch millis>}
```

A zero `device_id` means refusal. Then:

```
POST https://is.snssdk.com/service/settings/v3/?<common query + device_id>
x-ss-stub: <md5 of the literal body>
body: body=null
```

```json
{"data": {"settings": {"asr_config": {"app_key": "<token>"}}}}
```

Common query parameters: `device_platform=android`, `os=android`, `ssmix=a`,
`_rticket=<epoch millis>`, `cdid`, `channel`, `aid`, `app_name`, `version_code`,
`version_name`. For the register call they are followed by `manifest_version_code`,
`update_version_code`, `resolution`, `dpi`, `device_type`, `device_brand`, `language`,
`os_api`, `os_version`, `ac=wifi`; the settings call swaps those for `device_id`.

`ji` caches the identity plus token in `~/.local/state/ji/device.json`.

## Message envelope

Protobuf, written and read by hand (`src/pb.rs`). One message shape in both directions:

| Field | Wire | Meaning |
| --- | --- | --- |
| 2 | string | app key: the token on control messages, empty on audio frames |
| 3 | string | constant `ASR` |
| 5 | string | method name |
| 6 | string | payload, JSON encoded |
| 7 | bytes | one Opus packet |
| 8 | string | request id, one UUID per connection |
| 9 | varint | frame state, omitted when zero |

Response fields: 4 message type, 5 status code, 6 status message, 7 result JSON. Other
fields are skipped.

`frame_state` is `1` on the first audio frame, `3` on the middle ones, `9` on the last.

## Session

```
StartTask      payload empty            wait TaskStarted
StartSession   payload = session JSON   wait SessionStarted
TaskRequest    payload = {"extra":{},"timestamp_ms":<t0 + n*60>}
TaskRequest    payload = {"extra":{"finish_audio":true,"force_asr_twopass":true},"timestamp_ms":<...>}   frame_state 9
FinishSession  payload empty            wait SessionFinished
```

Any `TaskFailed` or `SessionFailed` is an error carrying the status code and message.
`timestamp_ms` advances 60 milliseconds per packet, which is also the send pacing. The
final frame must be valid Opus, so it is 960 samples of silence rather than a truncated
real frame.

The session config sent with `StartSession`:

```json
{"audio_info": {"channel": 1, "format": "speech_opus", "sample_rate": 16000},
 "enable_punctuation": true, "enable_speech_rejection": false,
 "extra": {"app_name": "com.android.chrome", "app_version": "1.1.2", "aid": "401734",
   "cell_compress_rate": 8, "did": "<device_id>", "enable_asr_threepass": true,
   "enable_asr_twopass": true, "enable_print_chinese": false,
   "disable_user_words": true, "enable_text_post_process": true,
   "asr_text_post_process_type": "last_post_process", "input_mode": "tool",
   "strong_ddc": true, "use_twopass_retry": true,
   "update_version_code": "100102018", "version_code": "100102018", "version_name": "1.1.2"}}
```

`disable_user_words` is true because the personal lexicon is out of scope: that feature
needs a `get_config` round trip, a P-256 `keyhub.zijieapi.com` handshake and a
ChaCha20 upload to `speech.bytedance.com/api/v3/context/ime/user_words`.

## Audio

16 kHz, mono, Opus, `Application::Voip`, one 60 ms packet per frame. A partial final
frame is padded to the next valid 20, 40 or 60 ms length before encoding. Packets go out
on a 60 ms wall-clock pace, one binary WebSocket message each, no length prefix.

## Reading results

```json
{"results": [{"text": "…", "index": 0, "start_time": 0, "end_time": 1200, "is_interim": false}]}
```

`is_interim` false is a settled segment: keep it, keyed by `index`. Anything else is a
partial: report it, keep it only as a fallback. A response whose result JSON has no
`results` array is a heartbeat.
