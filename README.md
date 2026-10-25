# Ji (记)

Voice input as a pipe: 16 kHz mono PCM in, transcript out. It records nothing on its own,
binds no hotkey and pastes into no text field, because those jobs belong to the programs you
already run.

```
$ timeout 10 pw-cat --record --container=raw --format=s16 --rate=16000 --channels=1 - | ji
The meeting today moved to three in the afternoon.
```

```
$ ji-dictate     # start dictating
$ ji-dictate     # press again to stop, and the answer is typed where you are
```

Installs give you both `ji` and `ji-dictate`.

## Install

```sh
brew tap zaypen/ji && brew install ji          # Apple Silicon
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/zaypen/ji/releases/latest/download/ji-installer.sh | sh
```

Both forms drop working commands on your `PATH`. The second is for anyone who would rather
not have Homebrew.

Linux and other machines build from source:

```sh
cargo install --path . --root ~/.local
install -m 755 contrib/ji-dictate ~/.local/bin/ji-dictate   # optional, see below
```

There is no Linux binary yet, and the reason is not laziness: libopus links dynamically on
glibc, so a binary out of a build machine dies at startup anywhere that does not already have
libopus installed. `nix run github:zaypen/ji` works today, and the flake builds against opus
properly.

Install `ji` as a real file rather than a link into `target/`. `cargo clean` then cannot leave
a dangling command on your `PATH`, and on macOS it matters twice: the microphone and
accessibility rights are attached to the executable's path and identity, so a path that
changes on every rebuild means granting permission again on every rebuild.

### What you need

| | `ji` on a pipe | `ji-dictate` at a hotkey |
| --- | --- | --- |
| build | `libopus`, `pkg-config` | same |
| record | your problem, or `ffmpeg` | `ffmpeg`, started for you |
| type it in | nothing, it prints | `wtype` on Wayland; nothing extra on macOS |
| make a sound | nothing | `pw-play` on Linux; `afplay` is already on macOS |

On Linux that is `ffmpeg` and `wtype`. macOS needs `ffmpeg` and nothing else for typing,
because the pasteboard, `osascript` and `afplay` are parts of the system.

Put libopus on the machine before you build it, because the fallback is bad: `audiopus_sys`
ships a libopus whose CMakeLists declares `cmake_minimum_required(VERSION 3.1)`, and CMake 4
dropped compatibility below 3.5, so a machine with no libopus cannot configure it. Install
libopus and let `pkg-config` find it; `cmake` will not save you.

## Use

### At a hotkey

```sh
ji-dictate                    # start
ji-dictate                    # stop, and the answer is typed
DICTATE_SECONDS=10 ji-dictate # ten seconds, for a one-press dictation
```

`ji-dictate` is `ffmpeg | ji | wtype` with the four things a shell one-liner gets wrong. It
signals the recorder and never the transcriber, so `ji` sees the stream end and prints while
the inserter is still awake to use what it printed. It keeps the two pids it needs in
`$XDG_RUNTIME_DIR/ji-dictate.pid` instead of guessing. It closes a dictation you forgot to end.
It answers out loud both times — rising tone to start, falling tone to end, plus what landed —
because a dictation you cannot tell apart from a hang is no use to anybody.

Knobs, all optional: `JI_BIN` (defaults to `ji` beside `ji-dictate`), `DICTATE_SECONDS`
(60), `DICTATE_DEVICE` (the input, defaulting to your system default device; pass an index or
a name, and see the choices with `ffmpeg -f avfoundation -list_devices true -i ""`).

The last one needs care: index `0` is the first device found, which on a machine with several
inputs is not the same device as the default one.

Bind the key in whatever sends keys on your machine. On niri that is dms, and the path has to
be absolute:

```sh
dms keybinds set niri Pause "spawn /home/you/.local/bin/ji-dictate" --desc "Dictate (ji)"
niri msg action load-config-file
```

### On a pipe

```
ji [-p provider] [-m model] [-v] audio-source
  -p, --provider <name>  doubaoime_android (default) or audiocpp
  -m, --model <id>       model id for the audiocpp provider (default qwen3-asr)
  -v, --verbose          one line: a spinner while it waits, then the level and the words so far
  -V, --version          print the version and stop
  -h, --help             show this message
```

Exit `0` and text on stdout for a dictation, `1` with a reason on stderr for everything else,
including a bad command line.

Input is fixed: 16 kHz, mono, signed 16-bit little-endian PCM, no container. If your source is
not that, `ffmpeg` converts it in front of `ji`:

```sh
ffmpeg -i meeting.m4a -ar 16000 -ac 1 -f s16le - | ji --provider audiocpp
```

What ends the pipe is what `ji` takes as "I have finished speaking", so the recorder has to
stop on its own terms: `ffmpeg -t 10`, `timeout 10`, `pw-cat --sample-count N`. `ji` also
answers the signals that end a recording — `SIGINT` from the keyboard, `SIGTERM` from
`timeout` and from shutdown — by finishing the run and printing what the provider already
heard, so a signal-stopped pipe still gives you a transcript.

Two spelling traps. PipeWire calls the sample format `s16` and `ffmpeg` calls the same bytes
`s16le`; mix them up and `pw-cat` says `unknown format`. And `pw-cat` writes a container to
stdout unless you ask for raw, so add `--container=raw`; without it you get a 24-byte `dns.`
header before the samples.

### Permissions on macOS

A command-line tool never appears in the privacy list, and `ji` did not get that wrong. Rights
attach to the responsible process, and for anything started from a shell that is the terminal,
so the terminal is asked once for Microphone and Accessibility and every command that can run
in that window holds both grants.

* The pasteboard needs no permission of its own; only the Cmd-V that follows it needs
  Accessibility. That is why the answer travels through the clipboard rather than through
  keystrokes a keyboard layout might not be able to express.
* A dictation with the grants missing fails quietly — nothing recorded, or the paste does
  nothing — and `ji-dictate` reports both. Check the terminal before blaming the script.
* If something is on your mind that the microphone should not catch, revoke it while you are
  not dictating: the switch in Settings covers the microphone, and `tccutil reset Accessibility
  <terminal bundle id>` brings the dialog back.

## Providers

| Provider | Where the audio goes | Notes |
| --- | --- | --- |
| `doubaoime_android` | Doubao's real-time ASR gateway, reverse engineered from the Android IME client | Default. No account, no key. A device identity is registered once and cached. Can stop working whenever the service changes. |
| `audiocpp` | A server you point `JI_AUDIOCPP_URL` at | The model's name decides the door: without `-stream` the audio is recorded and uploaded in one request, with it it is sent as captured and answered on the same connection. Requires `JI_AUDIOCPP_URL`. |

Environment defaults, overridden by flags:

| Variable | Default | Meaning |
| --- | --- | --- |
| `JI_PROVIDER` | `doubaoime_android` | Provider name |
| `JI_AUDIOCPP_URL` | none | Base URL of the audiocpp provider, required to use it |
| `JI_MODEL` | `qwen3-asr` | Model id for the audiocpp provider |
| `JI_DOUBAO_REGISTER` | `https://log.snssdk.com/...` | Where the first provider registers a device |
| `JI_DOUBAO_SETTINGS` | `https://is.snssdk.com/...` | Where it gets the recognition token |
| `JI_DOUBAO_GATEWAY` | `wss://frontier-audio-ime-ws.doubao.com/...` | Where the audio goes |

### Device identity

The `doubaoime_android` provider registers itself once as an Android phone so that Doubao will
serve it. `cdid`, `clientudid`, `openudid`, the returned device id and the recognition token
are cached in `~/.local/state/ji/device.json` (`XDG_STATE_HOME` is honoured) and reused later.
Delete that file to force a new identity. Nothing else about your machine is sent.

Audio sent to either provider is processed by that provider. If you would rather not hand
speech to ByteDance, run `ji --provider audiocpp` against a server you control.

## Non-goals

Hotkeys, tray icons, clipboard, typing into other windows, an input-method front end, daemon
mode, LLM cleanup of transcripts, and translation. Each is a separate tool that can pipe with
`ji`; none of them make this one smaller.

## Privacy and terms

The `doubaoime_android` provider is based on observed client behaviour, not on an official
public API, and it is not affiliated with, endorsed by or sponsored by ByteDance. Use it at
your own risk; the service may change authentication, the wire format, rate limits or access
policy at any time, and it may not allow this use. Audio you dictate is processed by whichever
provider you chose.

## Development

```sh
nix develop --command cargo test          # 26 tests, no network needed
nix develop --command cargo clippy --all-targets -- -D warnings
nix develop --command cargo fmt --check
nix shell nixpkgs#cargo-dist -c dist generate   # after changing dist-workspace.toml
```

Releases are tagged, not hand-built: a tag whose version matches `Cargo.toml` produces the
GitHub release, the installers and the Homebrew formula. `docs/design.md` explains the process
model and the provider seam, and `docs/protocol.md` documents the Doubao IME gateway wire
format.

## Why it is shaped this way

Chinese dictation on Linux is usually either a heavyweight input-method app or a cloud demo,
and neither is a good Unix citizen: it owns your input focus, hides the recognition behind a
GUI, and cannot be piped. `ji` is the middle piece of a pipeline, so you compose it:

```sh
ffmpeg -f pulse -i default -t 10 -ar 16000 -ac 1 -f s16le - | ji | wtype -
```

A tool that did the recording, the typing and the hotkey itself would be easier to start with
and impossible to reuse. It would also be the thing standing between you and the tool that
types the way you actually want.

## License

MIT. See `LICENSE`.
