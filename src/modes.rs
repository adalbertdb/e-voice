//! Processing profile for prompt construction.
//! `Mode` is a private implementation detail of the processor module.
//! It is not exposed in the public interface and will evolve into the Profile system.

use std::fmt::{Display, Formatter};
use std::str::FromStr;

const OUTPUT_SUFFIX: &str = "\nOutput only the processed text. No explanations, no quotes, no commentary.\n\nInput: \"{text}\"\nOutput:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Mode {
    Clean,
}

impl Mode {
    pub fn prompt_template(&self, text: &str) -> String {
        let instruction = match self {
            Mode::Clean => {
                "You are a text post-processor for speech-to-text output.\n\n\
                 First, check if the input ends with an explicit instruction:\n\
                 - \"translate to <language>:\" - translate the text into that language\n\
                 - \"make it formal:\" - rewrite in a professional, formal tone\n\
                 - \"make it casual:\" - rewrite in a friendly, natural tone\n\
                 - \"bullet points:\" or \"make it a list:\" - format as Markdown bullet points\n\n\
                 If an instruction is present, execute it and strip the instruction prefix from the output.\n\
                 If no instruction is present but the input naturally describes a list of items \
                 (e.g., \"buy milk, eggs, and bread\" or \"first do this, second do that\"), \
                 format it as Markdown bullet points.\n\
                 Otherwise, clean the transcribed speech by removing filler words, fixing punctuation, \
                 and preserving the original meaning."
            }
        };

        format!("{instruction}{}", OUTPUT_SUFFIX.replace("{text}", text))
    }
}

impl Display for Mode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Mode::Clean => write!(f, "clean"),
        }
    }
}

impl FromStr for Mode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.eq_ignore_ascii_case("clean") {
            Ok(Mode::Clean)
        } else {
            Err(format!("unsupported mode: {value}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Mode;
    use std::str::FromStr;

    #[test]
    fn prompt_template_is_non_empty() {
        let prompt = Mode::Clean.prompt_template("sample input");
        assert!(!prompt.trim().is_empty(), "prompt should not be empty");
    }

    #[test]
    fn parses_clean_mode() {
        let mode = Mode::from_str("clean").expect("clean should parse");
        assert_eq!(mode, Mode::Clean);
    }

    #[test]
    fn rejects_non_clean_modes() {
        let err = Mode::from_str("formal").expect_err("formal should fail");
        assert!(err.contains("unsupported mode"));
    }
}
