//! Processing profiles for prompt construction.
//!
//! `Profile` is the named prompt strategy tied to social context.  It replaces
//! the old `Mode::Clean` placeholder and is the primary public abstraction for
//! controlling how transcribed speech is rewritten by the LLM.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

const OUTPUT_SUFFIX: &str =
    "\nOutput only the processed text. No explanations, no quotes, no commentary.\
     \n\nInput: \"{text}\"\nOutput:";

/// A named prompt strategy.
///
/// Each variant encapsulates a distinct LLM instruction set.  The active
/// profile is stored in `AppState` and persisted across daemon restarts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Profile {
    /// Universal interpreter — clean speech, detect inline instructions.
    /// This is the default profile.
    UniversalInterpreter,
    /// Rewrite in a professional, formal tone.
    Formal,
    /// Rewrite in a relaxed, friendly tone.
    Casual,
    /// Format as Markdown bullet points.
    Bullet,
    /// Translate to the given target language (e.g. `"es"`, `"fr"`).
    Translate(String),
}

impl Default for Profile {
    fn default() -> Self {
        Profile::UniversalInterpreter
    }
}

impl Profile {
    /// Build the full LLM prompt by substituting `text` into the profile's
    /// template.
    pub fn prompt_for(&self, text: &str) -> String {
        let instruction: String = match self {
            Profile::UniversalInterpreter => {
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
                    .to_owned()
            }
            Profile::Formal => {
                "You are a professional writing assistant.\n\
                 Rewrite the following text in a formal, professional tone.\n\
                 Maintain the original meaning while improving clarity and register."
                    .to_owned()
            }
            Profile::Casual => {
                "You are a friendly writing assistant.\n\
                 Rewrite the following text in a casual, friendly, and relaxed tone.\n\
                 Keep it natural and conversational while preserving the original meaning."
                    .to_owned()
            }
            Profile::Bullet => {
                "You are a writing assistant that formats text as structured lists.\n\
                 Convert the following text into Markdown bullet points.\n\
                 Each distinct idea or item should become its own bullet point."
                    .to_owned()
            }
            Profile::Translate(lang) => {
                format!(
                    "You are a professional translator.\n\
                     Translate the following text into {lang}.\n\
                     Preserve the original meaning, tone, and formatting."
                )
            }
        };

        format!("{instruction}{}", OUTPUT_SUFFIX.replace("{text}", text))
    }

    /// Return the canonical string name of this profile (used for Display,
    /// serialization, and the HTTP/state-file representation).
    pub fn name(&self) -> String {
        self.to_string()
    }
}

// ─── Display / FromStr ────────────────────────────────────────────────────────

impl Display for Profile {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Profile::UniversalInterpreter => write!(f, "universal_interpreter"),
            Profile::Formal => write!(f, "formal"),
            Profile::Casual => write!(f, "casual"),
            Profile::Bullet => write!(f, "bullet"),
            Profile::Translate(lang) => write!(f, "translate:{lang}"),
        }
    }
}

impl FromStr for Profile {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "universal_interpreter" => Ok(Profile::UniversalInterpreter),
            "formal" => Ok(Profile::Formal),
            "casual" => Ok(Profile::Casual),
            "bullet" => Ok(Profile::Bullet),
            s if s.starts_with("translate:") => {
                let lang = s.trim_start_matches("translate:").to_owned();
                if lang.is_empty() {
                    Err("translate profile requires a target language".to_owned())
                } else {
                    Ok(Profile::Translate(lang))
                }
            }
            other => Err(format!("unknown profile: {other}")),
        }
    }
}

// ─── Serde ────────────────────────────────────────────────────────────────────

impl Serialize for Profile {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Profile {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Profile::from_str(&s).map_err(serde::de::Error::custom)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::Profile;
    use std::str::FromStr;

    // ── Domain model ──────────────────────────────────────────────────────────

    #[test]
    fn test_universal_interpreter_prompt_contains_instruction() {
        let prompt = Profile::UniversalInterpreter.prompt_for("hello");
        assert!(
            prompt.contains("post-processor") || prompt.contains("filler words"),
            "universal interpreter prompt should describe its cleaning behaviour; got:\n{prompt}"
        );
    }

    #[test]
    fn test_formal_prompt_requests_professional_tone() {
        let prompt = Profile::Formal.prompt_for("hey");
        let lower = prompt.to_lowercase();
        assert!(
            lower.contains("professional") || lower.contains("formal"),
            "formal prompt should mention professional/formal; got:\n{prompt}"
        );
    }

    #[test]
    fn test_casual_prompt_requests_relaxed_tone() {
        let prompt = Profile::Casual.prompt_for("hello");
        let lower = prompt.to_lowercase();
        assert!(
            lower.contains("casual") || lower.contains("friendly"),
            "casual prompt should mention casual/friendly; got:\n{prompt}"
        );
    }

    #[test]
    fn test_bullet_prompt_requests_list_format() {
        let prompt = Profile::Bullet.prompt_for("milk eggs bread");
        let lower = prompt.to_lowercase();
        assert!(
            lower.contains("bullet") || lower.contains("list"),
            "bullet prompt should mention bullet points/list; got:\n{prompt}"
        );
    }

    #[test]
    fn test_translate_prompt_contains_target_language() {
        let prompt = Profile::Translate("es".to_owned()).prompt_for("hello");
        assert!(
            prompt.contains("es"),
            "translate prompt should contain the target language code; got:\n{prompt}"
        );
    }

    #[test]
    fn test_prompt_substitutes_input_text() {
        let input = "MY_TEST_INPUT";
        let profiles = vec![
            Profile::UniversalInterpreter,
            Profile::Formal,
            Profile::Casual,
            Profile::Bullet,
            Profile::Translate("de".to_owned()),
        ];
        for profile in profiles {
            let prompt = profile.prompt_for(input);
            assert!(
                prompt.contains(input),
                "profile {profile} prompt should contain the input text; got:\n{prompt}"
            );
        }
    }

    #[test]
    fn test_profile_serialization_roundtrip() {
        let profiles = vec![
            Profile::UniversalInterpreter,
            Profile::Formal,
            Profile::Casual,
            Profile::Bullet,
            Profile::Translate("es".to_owned()),
        ];
        for profile in profiles {
            // JSON roundtrip
            let json = serde_json::to_string(&profile).expect("should serialize to JSON");
            let restored: Profile =
                serde_json::from_str(&json).expect("should deserialize from JSON");
            assert_eq!(profile, restored, "JSON roundtrip failed for {profile}");

            // TOML roundtrip via wrapper
            #[derive(serde::Serialize, serde::Deserialize)]
            struct Wrapper {
                profile: Profile,
            }
            let w = Wrapper {
                profile: profile.clone(),
            };
            let toml_str = toml::to_string(&w).expect("should serialize to TOML");
            let w2: Wrapper = toml::from_str(&toml_str).expect("should deserialize from TOML");
            assert_eq!(profile, w2.profile, "TOML roundtrip failed for {profile}");
        }
    }

    // ── FromStr / Display ─────────────────────────────────────────────────────

    #[test]
    fn parses_all_built_in_profiles() {
        assert_eq!(
            Profile::from_str("universal_interpreter").unwrap(),
            Profile::UniversalInterpreter
        );
        assert_eq!(Profile::from_str("formal").unwrap(), Profile::Formal);
        assert_eq!(Profile::from_str("casual").unwrap(), Profile::Casual);
        assert_eq!(Profile::from_str("bullet").unwrap(), Profile::Bullet);
        assert_eq!(
            Profile::from_str("translate:es").unwrap(),
            Profile::Translate("es".to_owned())
        );
    }

    #[test]
    fn display_roundtrips_via_from_str() {
        let profiles = vec![
            Profile::UniversalInterpreter,
            Profile::Formal,
            Profile::Casual,
            Profile::Bullet,
            Profile::Translate("fr".to_owned()),
        ];
        for p in profiles {
            let s = p.to_string();
            let restored = Profile::from_str(&s).expect("Display output should parse back");
            assert_eq!(p, restored);
        }
    }

    #[test]
    fn rejects_unknown_profile() {
        let err = Profile::from_str("foo").expect_err("unknown profile should fail");
        assert!(err.contains("unknown profile"));
    }

    #[test]
    fn default_profile_is_universal_interpreter() {
        assert_eq!(Profile::default(), Profile::UniversalInterpreter);
    }
}
