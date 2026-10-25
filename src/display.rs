use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

static VERBOSE: AtomicBool = AtomicBool::new(false);
static TTY: AtomicBool = AtomicBool::new(false);

/// Smoothed input level, kept between updates as the bits of an f32.
static LEVEL: AtomicU32 = AtomicU32::new(0);

const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// The newest status and the newest transcript text, so either can be updated alone.
static LINE: Mutex<Option<(String, String)>> = Mutex::new(None);

/// `-v` decides whether anything is shown. Whether it is shown in place depends on stderr
/// being a terminal, so piping the run to a log file stays readable.
pub fn configure(verbose: bool) {
  VERBOSE.store(verbose, Ordering::Relaxed);
  TTY.store(std::io::stderr().is_terminal(), Ordering::Relaxed);
}

pub fn enabled() -> bool {
  VERBOSE.load(Ordering::Relaxed)
}

/// A step of the run: the socket, the session, a device identity.
pub fn status(text: &str) {
  if enabled() {
    keep(text, &partial_text());
  }
}

/// The complete view of the dictation so far. Providers replace this wholesale, which is
/// why an in-place rewrite is correct: never append to it, never print only the delta.
pub fn partial(text: &str) {
  if enabled() {
    keep(&status_text(), text);
  }
}

/// The meter. One block per update, read from the loudest sample in what was just read.
/// It is also the evidence for the difference between a quiet room and a broken run.
pub fn level(samples: &[i16]) {
  if !enabled() {
    return;
  }
  let mut loudest = 0u16;
  for sample in samples {
    loudest = loudest.max(sample.unsigned_abs());
  }
  show(loudest)
}

/// Same thing for a batch provider that already holds a whole buffer of little-endian
/// samples, so it does not need to invent a conversion of its own.
pub fn level_from_bytes(bytes: &[u8]) {
  if enabled() {
    show(
      bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]).unsigned_abs())
        .max()
        .unwrap_or(0),
    );
  }
}

fn show(loudest: u16) {
  let level = (u32::from(loudest) as f32 / 32767.0).sqrt();
  let previous = f32::from_bits(LEVEL.swap(level.to_bits(), Ordering::Relaxed));
  // Rising is immediate, falling is decayed, so a word does not flash and vanish.
  let shown = level.max(previous * 0.7);
  LEVEL.store(shown.to_bits(), Ordering::Relaxed);
  keep(&status_text(), &partial_text());
}

/// Remove the live line before stdout prints the answer.
pub fn quiet() {
  if TTY.load(Ordering::Relaxed) {
    eprint!("\r\x1b[2K");
    let _ = std::io::stderr().flush();
  }
}

fn keep(status: &str, text: &str) {
  if let Ok(mut slot) = LINE.lock() {
    *slot = Some((status.to_owned(), text.to_owned()));
  }
  let meter = BLOCKS[((f32::from_bits(LEVEL.load(Ordering::Relaxed)) * 7.99) as usize).min(7)];
  let message = match (status.trim(), text.trim()) {
    ("", "") => String::new(),
    ("", text) => text.to_owned(),
    (status, "") => status.to_owned(),
    (status, text) => format!("{status} {text}"),
  };
  let rendered = if message.is_empty() {
    meter.to_string()
  } else {
    format!("{meter} {message}")
  };
  if TTY.load(Ordering::Relaxed) {
    eprint!("\r\x1b[2K{}", fit(&rendered, width()));
    let _ = std::io::stderr().flush();
  } else {
    eprintln!("{rendered}");
  }
}

fn status_text() -> String {
  LINE
    .lock()
    .ok()
    .and_then(|slot| slot.as_ref().map(|(status, _)| status.clone()))
    .unwrap_or_default()
}

fn partial_text() -> String {
  LINE
    .lock()
    .ok()
    .and_then(|slot| slot.as_ref().map(|(_, text)| text.clone()))
    .unwrap_or_default()
}

/// Terminal width, so a long dictation overwrites its own line instead of pushing text
/// down and leaving pieces of earlier updates behind.
fn width() -> usize {
  let mut size = libc::winsize {
    ws_col: 0,
    ws_row: 0,
    ws_xpixel: 0,
    ws_ypixel: 0,
  };
  let called = unsafe { libc::ioctl(libc::STDERR_FILENO, libc::TIOCGWINSZ, &mut size) };
  if called == 0 && size.ws_col > 1 {
    usize::from(size.ws_col)
  } else {
    80
  }
}

/// Whole characters only, counted in display columns because the transcript is Chinese.
fn fit(text: &str, columns: usize) -> String {
  let mut kept = String::new();
  let mut used = 0;
  for character in text.chars() {
    let cost = if character.len_utf16() > 1 || is_wide(character) {
      2
    } else {
      1
    };
    if used + cost > columns {
      return format!("{kept}…");
    }
    used += cost;
    kept.push(character);
  }
  kept
}

fn is_wide(character: char) -> bool {
  matches!(character as u32,
    0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF
      | 0xFE30..=0xFE6F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x20000..=0x3FFFD)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_long_view_is_cut_on_a_whole_character_and_never_on_a_byte() {
    let cut = fit("一二三四五六七八", 9);
    assert_eq!(cut, "一二三四…");
    assert!(std::str::from_utf8(cut.as_bytes()).is_ok());
  }

  #[test]
  fn silence_and_loud_speech_land_on_different_blocks() {
    assert_eq!(block_of(&[0i16, 1, -1]), '▁');
    assert_ne!(block_of(&[32000i16]), block_of(&[1i16]));
  }

  /// Independent of the smoothing between calls, which the live line also applies.
  fn block_of(samples: &[i16]) -> char {
    let loudest = samples.iter().map(|sample| sample.unsigned_abs()).max().unwrap_or(0);
    BLOCKS[((((u32::from(loudest) as f32) / 32767.0).sqrt() * 7.99) as usize).min(7)]
  }
}
