use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

static VERBOSE: AtomicBool = AtomicBool::new(false);
static TTY: AtomicBool = AtomicBool::new(false);

/// The live line is owned by the process, so it says when it stops being redrawn.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Whether any audio has been seen. Until it has, the line is a spinner and not a meter:
/// there is no level to show, and silence is not the same as waiting.
static SEEN: AtomicBool = AtomicBool::new(false);

/// The newest level, as the bits of an f32.
static LEVEL: AtomicU32 = AtomicU32::new(0);

static SPINNER: AtomicU32 = AtomicU32::new(0);

/// The newest view of the dictation, so the line can be redrawn without the provider being
/// asked again.
static VIEW: Mutex<String> = Mutex::new(String::new());

/// One line, so whoever is painting it is the only one allowed to.
static PAINT: Mutex<()> = Mutex::new(());

const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const FRAMES: [char; 8] = ['⣾', '⣽', '⣻', '⢿', '⡿', '⣟', '⣯', '⣷'];

/// `-v` decides whether anything is shown. Whether it is shown in place depends on stderr
/// being a terminal, so piping the run to a log file stays readable.
pub fn configure(verbose: bool) {
  VERBOSE.store(verbose, Ordering::Relaxed);
  TTY.store(std::io::stderr().is_terminal(), Ordering::Relaxed);
  if enabled() && TTY.load(Ordering::Relaxed) {
    RUNNING.store(true, Ordering::Relaxed);
    std::thread::spawn(tick);
  }
}

/// The clock the spinner needs: waiting is the one thing that changes nothing else, so
/// without a beat of its own a connection in progress looks exactly like a stopped process.
fn tick() {
  while RUNNING.load(Ordering::Relaxed) {
    std::thread::sleep(std::time::Duration::from_millis(100));
    SPINNER.fetch_add(1, Ordering::Relaxed);
    draw();
  }
}

fn enabled() -> bool {
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

/// The meter: the loudest sample in what was just read. It is also the evidence that tells
/// a quiet room from a dead pipe.
pub fn level(samples: &[i16]) {
  if enabled() {
    show(samples.iter().map(|sample| sample.unsigned_abs()).max().unwrap_or(0));
  }
}

/// The same for bytes that are already little-endian samples, so no provider has to write
/// a conversion of its own to report a level.
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
  SEEN.store(true, Ordering::Relaxed);
  LEVEL.store(
    (u32::from(loudest) as f32 / 32767.0).sqrt().to_bits(),
    Ordering::Relaxed,
  );
  draw();
}

/// Stop the line and remove it, before stdout prints the answer or stderr prints a failure.
/// Waiting for the lock means an in-flight repaint cannot land on top of what comes next.
pub fn quiet() {
  RUNNING.store(false, Ordering::Relaxed);
  let _guard = PAINT.lock().ok();
  if TTY.load(Ordering::Relaxed) {
    eprint!("\r\x1b[2K");
    let _ = std::io::stderr().flush();
  }
}

fn draw() {
  if !enabled() {
    return;
  }
  let Ok(_guard) = PAINT.lock() else { return };
  if !RUNNING.load(Ordering::Relaxed) {
    return;
  }
  let rendered = if SEEN.load(Ordering::Relaxed) {
    let block = BLOCKS[((f32::from_bits(LEVEL.load(Ordering::Relaxed)) * 7.99) as usize).min(7)];
    let view = VIEW.lock().map(|view| view.clone()).unwrap_or_default();
    if view.trim().is_empty() {
      block.to_string()
    } else {
      format!("{block} {}", fit(&view, width().saturating_sub(2)))
    }
  } else {
    FRAMES[SPINNER.load(Ordering::Relaxed) as usize % FRAMES.len()].to_string()
  };
  // The switch for when the line on screen looks wrong: it says exactly what ji painted,
  // which tells the reader whether ji wrote it or their terminal drew something else.
  if std::env::var("JI_TRACE").is_ok() {
    eprintln!("painted {rendered:?}");
  }
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
    assert_eq!(block(0), '▁');
    assert_ne!(block(32000), block(1));
    assert_eq!(block(u16::MAX), '█');
  }

  fn block(loudest: u16) -> char {
    BLOCKS[((((u32::from(loudest) as f32) / 32767.0).sqrt() * 7.99) as usize).min(7)]
  }
}
