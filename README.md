# 记 (Ji)

`记` is a command-line voice input tool for Linux. It reads 16 kHz mono PCM from
standard input, sends it to a speech recognizer, and prints the transcript on
standard output. It records nothing on its own, binds no hotkey, and pastes into no
text field. Those jobs belong to the programs you already run.

```
timeout 10 pw-cat --record --container=raw --format=s16 --rate=16000 --channels=1 - | ji
```

```
今天开会的时间改到下午三点。
```

One line in, one line out. Everything it knows is: read bytes, ask a provider, print
text.

The end of the pipe is what `记` takes as "I have finished speaking", so the recorder
has to stop: `ffmpeg -t N`, `timeout N`, `pw-cat --sample-count N`, or `Ctrl-C`. All of
them end up with a transcript, because `记` answers the signals that end a recording —
`SIGINT` from the keyboard, `SIGTERM` from `timeout` and from shutdown — by finishing the
dictation and printing what the gateway already heard. This matters in fish, where
`timeout` signals the whole pipeline rather than only the recorder. A signal-stopped run
gives the answer five seconds instead of the twenty used on a clean close, and the next
signal of the same kind reverts to the default action, so cleanup can never make the
process unkillable.

`-i default` is the current default source, which may be a webcam microphone rather than
the one you had in mind; find it with `wpctl status` and name the node you want.

## Why

Chinese dictation on Linux is usually either a heavyweight input-method app or a
cloud demo, and neither is a good Unix citizen: it owns your input focus, hides the
recognition behind a GUI, and cannot be piped. `记` is the middle piece of a pipeline,
so you compose it:

```sh
# dictate from a hotkey (niri example), ten seconds of speech
bind = Shift+V exec ffmpeg -f pulse -i default -t 10 -ar 16000 -ac 1 -f s16le - | ji | wtype -

# or with pw-cat, where something external has to end the recording
bind = Shift+V exec timeout 10 pw-cat --record --container=raw --format=s16 --rate=16000 --channels=1 - | ji | wtype -

# transcribe a file instead of the microphone
ffmpeg -i meeting.m4a -ar 16000 -ac 1 -f s16le - | ji --provider audiocpp
```

Both microphone forms need the recorder to stop on its own terms. `ffmpeg -t N` ends the
stream, so `记` gets a clean end of file. A timeout that signals the recorder does not:
the signal travels to the whole pipeline, `记` dies mid-run, and nothing is printed.

## Install

The project is a Nix flake; the toolchain comes from the shell, not from your system.

```sh
nix run .#                        # run the latest build
nix build                         # binary at result/bin/ji
nix develop --command cargo build --release
```

Without Nix, `cargo build --release` works as long as `libopus` and `pkg-config` are
present; the `opus` crate falls back to building libopus with CMake.

## Usage

```
pw-cat --record --container=raw --format=s16 --rate=16000 --channels=1 - | ji [options]
  -p, --provider <name>  doubaoime_android (default) or audiocpp
  -m, --model <id>       model id for the audiocpp provider (default qwen3-asr)
  -v, --verbose          one line: a spinner while it waits, then the input level and the words so far
  -h, --help             show this message
```

Exit codes: `0` and text on stdout for a successful dictation, `1` with a reason on
stderr for everything else, including a bad command line.

Input format is fixed: 16 kHz, mono, signed 16-bit little-endian PCM, no container.
Pipes are welcome; so is a WAV-less file. If your source is not that format, `ffmpeg`
or `pw-cat` converts it in front of `记`.

Two traps in the recorder's spelling. PipeWire calls the sample format `s16`, `ffmpeg`
calls the same bytes `s16le`, and `pw-cat` reports `unknown format` if you mix them up.
And `pw-cat` writes a container to stdout unless you ask for raw bytes, so
`pw-cat --record --format=s16 ... -` gives you a 24-byte `dns.` header before any
samples; use `--container=raw`, short form `-a`. Checked against `--sample-count=16000`:
the container-less forms return exactly 32000 bytes for 16000 samples, the default form
returns 32024.

### Providers

| Provider | Where the audio goes | Notes |
| --- | --- | --- |
| `doubaoime_android` | Doubao's real-time ASR gateway, reverse engineered from the Android IME client | Default. No account, no key. A device identity is registered once and cached. Can stop working whenever the service changes. |
| `audiocpp` | A server you point `JI_AUDIOCPP_URL` at | Which door the model's name opens: one without `-stream` is recorded and uploaded in one request, one with it is sent as it is captured and answered on the same connection. Needs `JI_AUDIOCPP_URL`. |

Environment defaults, overridden by flags:

| Variable | Default | Meaning |
| --- | --- | --- |
| `JI_PROVIDER` | `doubaoime_android` | Provider name |
| `JI_AUDIOCPP_URL` | none | Base URL of the audiocpp provider, required to use it |
| `JI_MODEL` | `qwen3-asr` | Model id for the audiocpp provider |

### Device identity

The `doubaoime_android` provider registers itself once as an Android phone so that Doubao will
serve it. `cdid`, `clientudid`, `openudid`, the returned device id and the recognition
token are cached in `~/.local/state/ji/device.json` (`XDG_STATE_HOME` is honoured)
and reused by later runs. Delete that file to force a new identity. Nothing else about
your machine is sent.

Audio sent to either provider is processed by that provider. If you would rather not
hand speech to ByteDance, run `ji --provider audiocpp` against a server you control.

## Non-goals

Hotkeys, tray icons, clipboard, typing into other windows, an input-method front end,
daemon mode, LLM cleanup of transcripts, and translation. Each is a separate tool that
can pipe with `记`; none of them make this one smaller.

## Privacy and terms

The `doubaoime_android` provider is based on observed client behaviour, not on an official
public API, and it is not affiliated with, endorsed by or sponsored by ByteDance.
Use it at your own risk; the service may change authentication, the wire format, rate
limits or access policy at any time, and it may not allow this use. Audio you dictate
is processed by whichever provider you chose.

## Development

```sh
nix develop --command cargo test      # 16 tests, no network needed
nix develop --command cargo clippy --all-targets
nix develop --command cargo fmt --check
```

`docs/design.md` explains the process model and the provider seam. `docs/protocol.md`
documents the Doubao IME gateway wire format.

## License

MIT. See `LICENSE`.
