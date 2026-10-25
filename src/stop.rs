use std::io::ErrorKind;
use std::sync::atomic::{AtomicI32, Ordering};

static SIGNAL: AtomicI32 = AtomicI32::new(0);

unsafe extern "system" fn arrive(signal: i32) {
  if SIGNAL.swap(signal, Ordering::Relaxed) != 0 {
    // Already wrapping up. The next one of these gets the default answer, so nothing
    // this program starts can be made unkillable by its own cleanup.
    libc::signal(signal, libc::SIG_DFL);
  }
}

/// Ctrl-C and `timeout` both mean "the dictation is over", not "destroy this process".
/// Installed without SA_RESTART so a blocked read returns EINTR instead of restarting.
pub fn install() {
  for signal in [libc::SIGINT, libc::SIGTERM] {
    unsafe {
      let mut action: libc::sigaction = std::mem::zeroed();
      action.sa_flags = 0;
      action.sa_sigaction = arrive as *const () as usize;
      libc::sigaction(signal, &action, std::ptr::null_mut());
    }
  }
}

pub fn stopped() -> bool {
  SIGNAL.load(Ordering::Relaxed) != 0
}

pub fn read_captured(audio: &mut dyn std::io::Read) -> std::io::Result<Vec<u8>> {
  read_captured_with(audio, stopped)
}

pub fn read_captured_with(audio: &mut dyn std::io::Read, stop: impl Fn() -> bool) -> std::io::Result<Vec<u8>> {
  let mut captured = Vec::new();
  let mut buffer = vec![0u8; 64 * 1024];
  loop {
    match audio.read(&mut buffer) {
      Ok(0) => break,
      Ok(read) => captured.extend_from_slice(&buffer[..read]),
      Err(error) if error.kind() == ErrorKind::Interrupted => {
        if stop() {
          break;
        }
      }
      Err(error) => return Err(error),
    }
  }
  Ok(captured)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn reading_stops_at_end_of_file() {
    let captured = read_captured_with(&mut &[1u8, 2, 3][..], || false).unwrap();
    assert_eq!(captured, vec![1, 2, 3]);
  }

  #[test]
  fn a_signal_ends_the_read_and_keeps_what_arrived() {
    use std::io::Cursor;
    struct Source {
      data: Cursor<Vec<u8>>,
      served: bool,
    }

    impl std::io::Read for Source {
      fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.served {
          return Err(std::io::Error::from(ErrorKind::Interrupted));
        }
        self.served = true;
        self.data.read(buffer)
      }
    }

    let mut source = Source {
      data: Cursor::new(vec![7u8, 8]),
      served: false,
    };
    assert_eq!(read_captured_with(&mut source, || true).unwrap(), vec![7, 8]);
  }

  #[test]
  fn an_interrupt_before_any_sample_is_not_an_error() {
    struct Stuck;
    impl std::io::Read for Stuck {
      fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::from(ErrorKind::Interrupted))
      }
    }
    let captured = read_captured_with(&mut Stuck, || true).unwrap();
    assert_eq!(captured, Vec::<u8>::new());
  }
}
