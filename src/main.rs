mod audiocpp;
mod display;
mod doubaoime_android;
mod pb;
mod provider;
mod stop;

use std::io::{BufReader, IsTerminal, Read};
use std::process::ExitCode;

use crate::provider::{AsrResult, Provider};

const DEFAULT_MODEL: &str = "qwen3-asr";
const USAGE: &str = "usage: ffmpeg -f pulse -i default -t 10 -ar 16000 -ac 1 -f s16le - | ji [options]\n       or: timeout 10 pw-cat --record --container=raw --format=s16 --rate=16000 --channels=1 - | ji\n\nThe transcript is printed when the recording ends, and a signal that ends it works too:\nthe dictation is finished and whatever was heard is printed. Another signal of the same\nkind still kills the process.\n\noptions:
  -p, --provider <name>  doubaoime_android (default) or audiocpp
  -m, --model <id>       model id for the audiocpp provider
  -v, --verbose          show the input level and the current text, updated in place
  -h, --help             show this message";

fn main() -> ExitCode {
  match run(std::env::args().skip(1).collect()) {
    Ok(text) => {
      println!("{text}");
      ExitCode::SUCCESS
    }
    Err(error) => {
      eprintln!("ji: {error}");
      ExitCode::FAILURE
    }
  }
}

#[derive(Debug)]
struct Options {
  provider: String,
  base_url: Option<String>,
  model: String,
  verbose: bool,
}

impl Default for Options {
  fn default() -> Self {
    Options {
      provider: std::env::var("JI_PROVIDER").unwrap_or_else(|_| "doubaoime_android".into()),
      base_url: std::env::var("JI_AUDIOCPP_URL").ok(),
      model: std::env::var("JI_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into()),
      verbose: false,
    }
  }
}

fn run(arguments: Vec<String>) -> AsrResult<String> {
  stop::install();
  let options = parse(&arguments)?;
  display::configure(options.verbose);
  if std::io::stdin().is_terminal() {
    return Err(format!("no audio on stdin.\n{USAGE}").into());
  }
  let provider = select(&options)?;
  let audio = BufReader::with_capacity(64 * 1024, std::io::stdin());
  let mut report = |text: &str| display::partial(text);
  let answer = transcribe(provider.as_ref(), audio, &mut report);
  display::quiet();
  answer
}

fn parse(arguments: &[String]) -> AsrResult<Options> {
  let mut options = Options::default();
  let mut cursor = 0;
  while cursor < arguments.len() {
    let flag = arguments[cursor].as_str();
    cursor += 1;
    match flag {
      "-v" | "--verbose" => options.verbose = true,
      "-h" | "--help" => {
        println!("{USAGE}");
        std::process::exit(0);
      }
      "-p" | "--provider" => options.provider = value(arguments, &mut cursor, flag)?,
      "-m" | "--model" => options.model = value(arguments, &mut cursor, flag)?,
      other if other.starts_with('-') => {
        return Err(format!("unknown option {other}.\n{USAGE}").into());
      }
      other => return Err(format!("unexpected argument {other}.\n{USAGE}").into()),
    }
  }
  Ok(options)
}

fn value(arguments: &[String], cursor: &mut usize, flag: &str) -> AsrResult<String> {
  let Some(text) = arguments.get(*cursor).cloned() else {
    return Err(format!("{flag} needs a value.\n{USAGE}").into());
  };
  *cursor += 1;
  Ok(text)
}

fn select(options: &Options) -> AsrResult<Box<dyn Provider>> {
  match options.provider.as_str() {
    "doubaoime_android" => Ok(Box::new(doubaoime_android::DoubaoIme)),
    "audiocpp" => Ok(Box::new(audiocpp::Audiocpp {
      base: options
        .base_url
        .clone()
        .ok_or("the audiocpp provider needs JI_AUDIOCPP_URL, which has no default")?,
      model: options.model.clone(),
    })),
    other => Err(format!("unknown provider {other}, expected doubaoime_android or audiocpp.\n{USAGE}").into()),
  }
}

fn transcribe(provider: &dyn Provider, mut audio: impl Read, log: &mut dyn FnMut(&str)) -> AsrResult<String> {
  let text = provider.transcribe(&mut audio, log)?.trim().to_owned();
  if text.is_empty() {
    return Err("the gateway heard nothing".into());
  }
  Ok(text)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn flags_override_the_environment_defaults() {
    let options = parse(&["--provider".into(), "audiocpp".into(), "-v".into()]).unwrap();
    assert_eq!(options.provider, "audiocpp");
    assert!(options.verbose);
  }

  #[test]
  fn a_flag_without_a_value_is_refused() {
    assert!(parse(&["--provider".into()]).is_err());
  }

  #[test]
  fn an_unknown_provider_is_named_in_the_error() {
    let choice = select(&Options {
      provider: "whisper".into(),
      ..Options::default()
    });
    match choice {
      Ok(_) => panic!("whisper should not resolve to a provider"),
      Err(error) => assert!(error.to_string().contains("whisper")),
    }
  }
}
