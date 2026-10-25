use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static VERBOSE: AtomicBool = AtomicBool::new(false);
static TTY: AtomicBool = AtomicBool::new(false);

const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// The last few levels, oldest first. A strip rather than one block, because one block that
/// comes and goes says nothing about whether audio is arriving: silence and a stuck recorder
/// both look like a flat line, and so does a room where nobody has started speaking.
static STRIP: Mutex<Vec<char>> = Mutex::new(Vec::new());
const CELLS: usize = 12;

/// The newest view of the dictation, so the live line can be redrawn without the provider
/// being asked again.
static VIEW: Mutex<String> = Mutex::new(String::new());

/// `-v` decides whether anything is shown. Whether it is shown in place depends on stderr
/// being a terminal, so piping the run to a log file stays readable.
pub fn configure(verbose: bool) {
  VERBOSE.store(verbose, Ordering::Relaxed);
  TTY.store(std::io::stderr().is_terminal(), Ordering::Relaxed);
}

pub fn enabled() -> bool {
  VERBOSE.load(Ordering::Relaxed)
}

/// The complete view of the dictation so far. Providers replace this wholesale, which is
/// why an in-place rewrite is correct: never append to it, never print only the delta.
pub fn partial(text: &str) {
  if enabled() {
    if let Ok(mut view) = VIEW.lock() {
      *view = text.to_owned();
    }
    draw();
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
  if let Ok(mut strip) = STRIP.lock() {
    if strip.len() == CELLS {
      strip.remove(0);
    }
    strip.push(cell(loudest));
  }
  draw();
}

/// The block for one reading: squared amplitude, because the ear and the meter both read
/// loudness roughly that way, not linearly.
fn cell(loudest: u16) -> char {
  let level = (u32::from(loudest) as f32 / 32767.0).sqrt();
  BLOCKS[((level * 7.99) as usize).min(7)]
}

/// Remove the live line before stdout prints the answer.
pub fn quiet() {
  if TTY.load(Ordering::Relaxed) {
    eprint!("\r\x1b[2K");
    let _ = std::io::stderr().flush();
  }
}

/// The recent levels in brackets, so none of it reads like part of the sentence, and the
/// words heard so far. Rewritten in place on one line.
fn draw() {
  let view = VIEW.lock().map(|view| view.clone()).unwrap_or_default();
  let strip = STRIP
    .lock()
    .map(|strip| {
      let mut cells = "▁".repeat(CELLS.saturating_sub(strip.len()));
      cells.extend(strip.iter().copied());
      cells
    })
    .unwrap_or_default();
  let rendered = if view.trim().is_empty() {
    format!("[{strip}]")
  } else {
    format!("[{strip}] {}", fit(&view, width().saturating_sub(CELLS + 3)))
  };
  if TTY.load(Ordering::Relaxed) {
    eprint!("\r\x1b[2K{rendered}");
    let _ = std::io::stderr().flush();
  } else {
    eprintln!("{rendered}");
  }
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
    assert_eq!(cell(0), '▁');
    assert_ne!(cell(32000), cell(1));
    assert_eq!(cell(u16::MAX), '█');
  }
}
