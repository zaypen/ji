# Design

## The one decision that shapes everything

`记` reads a byte stream, calls one provider, prints one string. Nothing else.

```
stdin (16k mono s16le PCM)
  │  read_frame: 960 samples = 60 ms
  ▼
Provider::transcribe ──► provider wire format ──► transcript (String)
  │                                               ▲
  └── partial text ──► stderr (--log) ────────────┘
```

That boundary is the whole product: recording is somebody else's job, and so is
insertion. It is what lets `pw-cat | ji | wtype` work while `ji` stays small.

The cost of the choice is that the process starts cold, paying TLS and handshake costs
on every dictation. A resident daemon would avoid that and cost a socket, a lifecycle
and a concurrency story. Not worth it for the first version.

## Structure

| Module | Job |
| --- | --- |
| `main.rs` | Command line, stdin, provider choice, exit status. The only place that prints the answer. |
| `provider.rs` | `trait Provider` and the `AsrResult` alias. Three lines of contract, no behaviour. |
| `doubaoime_android.rs` | Doubao IME path: device identity, registration, token, WebSocket session, Opus, pacing, result assembly. |
| `audiocpp.rs` | The second route: waits for EOF, puts a WAV header on the bytes, uploads one multipart request, reads either one JSON answer or a stream of events. |
| `pb.rs` | Hand-written protobuf reader/writer for the gateway's message envelope. |

`Provider::transcribe` takes a reader plus a callback for interim text and returns the
final text. Providers decide whether they stream or batch; the caller does not care.

```rust
pub trait Provider {
  fn transcribe(&self, audio: &mut dyn Read, partial: &mut dyn FnMut(&str)) -> AsrResult<String>;
}
```

Adding a third provider means one module that implements this trait and one `match` arm
in `select`. Nothing else needs to change, which is the whole reason the trait exists.

## Why these internals

**Fixed input format.** 16 kHz mono `s16le` is exactly what both providers want, so no
resampling code exists. Anything else is converted by `ffmpeg` or `pw-cat` upstream,
which are better at that job than a Rust binary this size will ever be. The two spell
that format name differently: PipeWire says `s16`, `ffmpeg` says `s16le`.

**End of file is the end of the dictation.** A pipe that stays open keeps `记` waiting,
so the recorder has to be bounded: `ffmpeg -t N`, `timeout N`, `pw-cat --sample-count N`,
or `Ctrl-C`. Because the signal that ends a recording usually reaches `记` as well — in
fish, `timeout N recorder | 记` signals the whole pipeline — `记` catches `SIGINT` and
`SIGTERM` and turns them into end of file. Without that, the partial transcript the
provider had already produced dies with the process, which is what happened before.

The policy on `SIGTERM` is the part worth stating, because catching it is only polite if
the cleanup is bounded:

- one signal stops the reading, sends the closing frame and asks for the final answer,
  waiting five seconds rather than the twenty used on a clean close, so a machine that is
  shutting down is not held up by us;
- the next signal of the same kind restores the default disposition, so `Ctrl-C` twice or
  `kill` twice always works and the cleanup can never make the process unkillable;
- `SIGKILL` is not attempted: there is nothing to catch.

`SIGPIPE` and the rest are left alone, so a broken pipe still behaves the way it always
did.

**Two doors, named by the model.** A model whose name does not end in `-stream` gets the
upload: wait for EOF, put a WAV header on the bytes, send one multipart request, read one
JSON answer. A model whose name ends in `-stream` gets the live door, where the request body
*is* the recording — raw PCM, sent as it arrives, transcript coming back on the same
connection. That is the door that can answer before the speaker has finished, and it needs
no container and no multipart body, so there is nothing assembled that can be wrong. The
parameters in its query string are a promise the stream cannot check: `sample_rate=16000` is
declared, not verified, so audio that is really 48 kHz produces a confident wrong transcript
instead of an error. `ji` declares what it always accepts, which closes that hole.

**The shape of a partial answer.** A server that sends pieces uses `transcript.text.delta`
events that *continue* each other, plus one `transcript.text.done` carrying the whole thing,
punctuated. This is why the progress display can rewrite a single line instead of scrolling,
and why it never appends a piece to the previous one: pieces are joined here, not there. A
model that revises its own half-spoken sentence sends `transcript.text.partial` for the part
still open, and that part *replaces* — it must not be joined to what is already committed,
which is the one rule here whose failure would show up in the output as the same words
printed twice.

**Hand-written protobuf.** The gateway protocol has no published `.proto`. The client
sends one message shape (field 2 app key, 3 `ASR`, 5 method, 6 payload, 7 audio, 8
request id, 9 frame state) and receives one response shape (4 type, 5 status, 6 reason,
7 result JSON). Generating a `.proto` and a `prost` dependency for two messages would
buy nothing; the varint handling in `pb.rs` is debuggable and dependency free. Unknown
fields are skipped, so the format can grow without breaking the client.

**Opus in-process.** The gateway accepts 16 kHz Opus packets, so `记` encodes. PCM over
the wire would be 8× the bytes and is not what the IME client sends. The `opus` crate
links libopus through `audiopus_sys`, which is why the dev shell carries `opus`,
`pkg-config` and `cmake`.

**Wall-clock pacing.** One 60 ms packet is sent, then the process sleeps until
`t0 + n × 60 ms`. From a microphone nothing is late; from a file the audio is paced as
though it were being spoken. Sending as fast as the pipe can deliver makes the gateway
drop sessions, and a client that reads a file at 200× real time looks identical to a
client that misbehaves.

**Result assembly.** The server pushes interim and final segments with `index`,
`start_time` and `is_interim`. Finals go into a map keyed by index and are joined in
that order; interims are handed to the callback and kept only as a fallback when no
final segment ever arrives. This is why a long dictation cannot interleave text: the
map, not the arrival order, fixes the sentence order.

**Device identity.** Registered as a Pixel 7 Pro running the Doubao IME, because that
is what the gateway expects to see. The identity is cached so repeat runs look like the
same handset. Re-registering on every run is the fastest way to be rate limited.

## Error handling

Everything is `Result<_, Box<dyn Error>>`. Failures surface in `main`, which writes
`ji: <reason>` to stderr and exits non-zero. There is no silent fallback: an empty
transcript is an error, not a successful run with empty text, because the caller's
pipeline can then notice that the microphone or the service failed.

HTTP calls retry once. The network between this machine and ByteDance answers 504 often
enough that a single attempt fails for a reason no user can act on.

## Testing

16 tests, none of which touch the network:

- protobuf golden bytes for both directions, including skipping an unknown field;
- WAV header layout for the container the audiocpp provider must produce;
- partial reads of audio padded up to the next valid Opus frame, and EOF;
- transcript assembly for interim versus final segments;
- entropy: one `read_exact` from `/dev/urandom` rather than a read-until-EOF loop
  (this bug once hung the binary forever — `std::fs::read` never returns on
  `/dev/urandom`);
- command line parsing and provider selection.

Live behaviour is verified by hand against both providers:

```sh
ffmpeg -i sample.wav -ar 16000 -ac 1 -f s16le - | ji --provider audiocpp
ffmpeg -i sample.wav -ar 16000 -ac 1 -f s16le - | ji --provider doubaoime_android --log
```

## Not here

Recording devices, hotkeys, clipboard, typing into windows, daemon mode, batch of many
files, LLM post-correction of transcripts, translation, the Doubao personal lexicon,
the official Volcengine API (paid, needs a console key), and the other reverse-engineered
routes Douvo offers (Doubao Web, Youdao, Chatterfly).

If one of them arrives, it arrives as another `Provider`, and this document gets a line.
