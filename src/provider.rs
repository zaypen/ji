use std::io::Read;

pub type AsrResult<T> = Result<T, Box<dyn std::error::Error>>;

pub trait Provider {
  fn transcribe(&self, audio: &mut dyn Read, partial: &mut dyn FnMut(&str)) -> AsrResult<String>;
}
