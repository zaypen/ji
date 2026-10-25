# Ji (记)

`ji` is a command-line voice input tool for Linux. It reads 16 kHz mono PCM from
standard input, sends it to a speech recognizer, and prints the transcript on
standard output. It records nothing on its own, binds no hotkey, and pastes into no
text field. Those jobs belong to the programs you already run.

```
$ timeout 10 pw-cat --record --container=raw --format=s16 --rate=16000 --channels=1 - 2>/dev/null | ji
The meeting today moved to three in the afternoon.
```

One line in, one line out. Everything it knows is: read bytes, ask a provider, print
text.

The end of the pipe is what `ji` takes as "I have finished speaking", so the recorder
has to stop: `ffmpeg -t N`, `timeout N`, `pw-cat --sample-count N`, or `Ctrl-C`. All of
them end up with a transcript, because `ji` answers the signals that end a recording —
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
recognition behind a GUI, and cannot be piped. `ji` is the middle piece of a pipeline,
so you compose it:

```sh
# dictate from a hotkey (niri example), ten seconds of speech
bind = Shift+V exec ffmpeg -f pulse -i default -t 10 -ar 16000 -ac 1 -f s16le - | ji | wtype -

# or with pw-cat, where something external has to end the recording
bind = Shift+V exec timeout 10 pw-cat --record --container=raw --format=s16 --rate=16000 --channels=1 - 2>/dev/null | ji | wtype -

# transcribe a file instead of the microphone
ffmpeg -i meeting.m4a -ar 16000 -ac 1 -f s16le - | ji --provider audiocpp
```

Both microphone forms need the recorder to stop on its own terms. `ffmpeg -t N` ends the
stream, so `ji` gets a clean end of file. A timeout that signals the recorder does not:
the signal travels to the whole pipeline, `ji` dies mid-run, and nothing is printed.

`pw-cat` leaves one sign that it was ended rather than that it failed: a lone `-` on
stderr, which is its own progress output cut off. Silence it with `2>/dev/null` if it
bothers you, knowing that hides whatever else the recorder might have said.

## Install

The shortest path is the one that does not compile anything, for Apple Silicon Macs:

```sh
brew tap zaypen/ji && brew install ji
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/zaypen/ji/releases/latest/download/ji-installer.sh | sh
```

Either form drops a working `ji` on your `PATH`; the second is for people who would rather
not have Homebrew. Both install `ji-dictate` next to it, so nothing else is needed before the
first dictation. Linux has no prebuilt binary, and the reason is in the build settings
below: libopus would have to already be installed for the binary to run.

The binary is `ji`. `contrib/ji-dictate` is the glue a hotkey presses, and it is optional; both
land in `~/.local/bin`, which your `PATH` already looks at.

```sh
cargo install --path . --root ~/.local
install -m 755 contrib/ji-dictate ~/.local/bin/ji-dictate
```

`ji` goes in as a real file, not a link into `target/`. `cargo clean` then cannot leave a
dangling command on your `PATH`, and on macOS the same choice matters twice: microphone and
accessibility rights are attached to the executable's path and identity, so a path that
changes on every rebuild means granting permission again on every rebuild.

`libopus` and `pkg-config` are what the build needs; on macOS, `brew install opus
pkg-config`. Put those on the machine before you build, because the fallback is bad: the
`opus` crate bundles a libopus whose CMakeLists declares `cmake_minimum_required(VERSION
3.1)`, and CMake 4 removed compatibility below 3.5, so a machine with no libopus installed
cannot configure it. The release CI installs opus from Homebrew for exactly this reason.

The project is also a Nix flake, if you would rather not own a toolchain:

```sh
nix build                                              # binary at result/bin/ji
nix develop --command cargo install --path . --root ~/.local
```

### Pressing a key

```sh
ji-dictate     # start dictating
ji-dictate     # run it again to stop; the answer is typed into what is focused
```

`ji-dictate` is the composition from the section above, plus the four things a shell one-liner
cannot get right. It signals the recorder and never the transcriber, so `ji` sees the
stream end and prints while the inserter is still alive. It records the pids it needs in
`$XDG_RUNTIME_DIR/ji-dictate.pid` instead of guessing. It stops on its own after `DICTATE_SECONDS` if
you never press the key. It answers out loud on both presses, with a rising tone to start
and a falling one to end plus what landed, because a dictation you cannot tell apart from a
hang is no use to anybody.

The knobs are `JI_BIN`, `DICTATE_SECONDS`, and `DICTATE_DEVICE`, which selects the input on
either system and defaults to the system default device. Pass it an index or a device name;
for the macOS choices, run `ffmpeg -f avfoundation -list_devices true -i ""`. Index `0` is
the first device found, not the default one, so set it only if you actually meant one.

### Permissions on macOS

A command-line tool never appears in the privacy list, and this is not something `ji` got
wrong. Rights attach to the responsible process, and for anything started from a shell that
is the terminal, so `ji` is asked for nothing; the terminal is asked once, and every command
that can run in that window has both Microphone and Accessibility for as long as the grant
stands. Tools of this shape all behave this way.

So treat the grants as decisions rather than as setup:

* The pasteboard needs no permission of its own. Only the Cmd-V that follows it needs
  Accessibility, which is why the answer travels through the clipboard instead of through
  keystrokes a keyboard layout might not be able to express.
* The first dictation fails quietly if the grants are missing: the recorder returns nothing or
  the paste does nothing, and `ji-dictate` reports both. Check Microphone and Accessibility
  for the terminal before blaming the script.
* If something is on your mind that the microphone should not catch, take the grant away
  while you are not dictating. The switch in Settings covers the microphone, and
  `tccutil reset Accessibility <terminal bundle id>` puts the Accessibility dialog back.
* Everything you grant the terminal is available to whatever it runs. That is a fair thing to
  mind, and the honest answer is that a script in a terminal cannot fix it. The alternative is
  an application bundle, which is where the microphone dialog would name `ji-dictate` instead
  of your terminal. It is not something this project ships, because it is a second thing to
  build and trust, and it does not remove the exposure, only moves it.

Bind the key in whatever sends keys on your machine. On niri that is dms, and the path has
to be absolute:

```sh
dms keybinds set niri F13 "spawn /home/you/.local/bin/ji-dictate" --desc "Dictate (ji)"
niri msg action load-config-file
```

## Usage

```
pw-cat --record --container=raw --format=s16 --rate=16000 --channels=1 - | ji [options]
  -p, --provider <name>  doubaoime_android (default) or audiocpp
  -m, --model <id>       model id for the audiocpp provider (default qwen3-asr)
  -v, --verbose          one line: a spinner while it waits, then the input level and the words so far
  -V, --version          print the version and stop
  -h, --help             show this message
```

Exit codes: `0` and text on stdout for a successful dictation, `1` with a reason on
stderr for everything else, including a bad command line.

Input format is fixed: 16 kHz, mono, signed 16-bit little-endian PCM, no container.
Pipes are welcome; so is a WAV-less file. If your source is not that format, `ffmpeg`
or `pw-cat` converts it in front of `ji`.

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
| `JI_DOUBAO_REGISTER` | `https://log.snssdk.com/...` | Where the first provider registers a device |
| `JI_DOUBAO_SETTINGS` | `https://is.snssdk.com/...` | Where it gets the recognition token |
| `JI_DOUBAO_GATEWAY` | `wss://frontier-audio-ime-ws.doubao.com/...` | Where the audio goes |

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
can pipe with `ji`; none of them make this one smaller.

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
