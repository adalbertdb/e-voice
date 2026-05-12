//! Processing mode definitions and mode-specific prompt templates.

use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use thiserror::Error;

const OUTPUT_SUFFIX: &str = "\nOutput only the processed text. No explanations, no quotes, no commentary.\n\nInput: \"{text}\"\nOutput:";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Clean,
    Formal,
    Casual,
    Bullet,
    Translate(String),
}

impl Mode {
    pub fn prompt_template(&self, text: &str) -> String {
        let instruction = match self {
            Mode::Clean => {
                "You clean transcribed speech text by removing filler words, fixing punctuation, and preserving original meaning."
            }
            Mode::Formal => {
                "You rewrite transcribed speech into a polished, professional, and formal tone while preserving meaning."
            }
            Mode::Casual => {
                "You rewrite transcribed speech into a friendly, natural, and casual tone while preserving meaning."
            }
            Mode::Bullet => {
                "You transform transcribed speech into concise Markdown bullet points while preserving key details."
            }
            Mode::Translate(lang) => {
                return format!(
                    "You translate transcribed speech into {lang}, preserving intent and tone.{}",
                    OUTPUT_SUFFIX.replace("{text}", text)
                );
            }
        };

        format!("{instruction}{}", OUTPUT_SUFFIX.replace("{text}", text))
    }

    #[allow(dead_code)]
    pub fn as_key(&self) -> &'static str {
        match self {
            Mode::Clean => "clean",
            Mode::Formal => "formal",
            Mode::Casual => "casual",
            Mode::Bullet => "bullet",
            Mode::Translate(_) => "translate",
        }
    }
}

impl Display for Mode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Mode::Clean => write!(f, "clean"),
            Mode::Formal => write!(f, "formal"),
            Mode::Casual => write!(f, "casual"),
            Mode::Bullet => write!(f, "bullet"),
            Mode::Translate(lang) => write!(f, "translate:{lang}"),
        }
    }
}

#[derive(Debug, Error)]
pub enum ModeParseError {
    #[error("unsupported mode: {0}")]
    Unsupported(String),
    #[error("translate mode requires a target language, e.g. translate:es")]
    MissingLanguage,
}

impl FromStr for Mode {
    type Err = ModeParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.eq_ignore_ascii_case("clean") {
            return Ok(Mode::Clean);
        }
        if value.eq_ignore_ascii_case("formal") {
            return Ok(Mode::Formal);
        }
        if value.eq_ignore_ascii_case("casual") {
            return Ok(Mode::Casual);
        }
        if value.eq_ignore_ascii_case("bullet") {
            return Ok(Mode::Bullet);
        }

        if let Some((prefix, lang)) = value.split_once(':')
            && prefix.eq_ignore_ascii_case("translate")
        {
            let trimmed = lang.trim();
            if trimmed.is_empty() {
                return Err(ModeParseError::MissingLanguage);
            }
            return Ok(Mode::Translate(trimmed.to_owned()));
        }

        if value.eq_ignore_ascii_case("translate") {
            return Err(ModeParseError::MissingLanguage);
        }

        Err(ModeParseError::Unsupported(value.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::Mode;
    use std::str::FromStr;

    #[test]
    fn prompt_templates_are_non_empty_for_all_modes() {
        let modes = [
            Mode::Clean,
            Mode::Formal,
            Mode::Casual,
            Mode::Bullet,
            Mode::Translate("es".to_owned()),
        ];

        for mode in modes {
            let prompt = mode.prompt_template("sample input");
            assert!(
                !prompt.trim().is_empty(),
                "prompt for {mode} should not be empty"
            );
        }
    }

    #[test]
    fn parses_translate_mode_with_language() {
        let mode = Mode::from_str("translate:es").expect("translate:es should parse");
        assert_eq!(mode, Mode::Translate("es".to_owned()));
    }

    #[test]
    fn parses_clean_mode() {
        let mode = Mode::from_str("clean").expect("clean should parse");
        assert_eq!(mode, Mode::Clean);
    }
}
