// ji-dictate is contrib/ji-dictate, delivered inside a binary.
//
// Installers install binaries, not files, so a shell script that lives outside the crate is
// invisible to dist, to the Homebrew formula, and to anything else that installs rather than
// clones. This exists so those things can install it.
//
// It is still one implementation of the glue, not two: `bash -c` takes the script as one of
// its arguments, so nothing here re-parses it, stores it on disk, or keeps a second copy of
// the logic. bash reads the script out of argv, which is all it ever did out of a file.
fn main() {
  #[cfg(unix)]
  {
    use std::os::unix::process::CommandExt;
    let script = include_str!("../contrib/ji-dictate");
    let error = std::process::Command::new("bash")
      .arg("-c")
      .arg(script)
      .args(std::env::args_os().skip(1))
      .exec();
    // Only reached if bash is not there, and the script is nothing without it.
    eprintln!("ji-dictate: cannot start the shell that runs it: {error}");
    std::process::exit(1);
  }
  // Command::exec is unix-only, and so is everything the script goes on to do.
  #[cfg(not(unix))]
  {
    eprintln!("ji-dictate: dictation is only wired up for unix");
    std::process::exit(1);
  }
}
