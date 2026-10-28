//! Application internationalization (GUI and CLI).
//!
//! Source strings in the code act as translation keys (gettext-style). The
//! Spanish source is returned when a key is missing. Dictionaries are flat
//! `key -> translation` JSON files with optional metadata:
//!
//! ```json
//! { "_code": "fr", "_name": "Français", "Inicio": "Accueil" }
//! ```
//!
//! Built-in Spanish and English are embedded. Any other `*.json` placed in an
//! `i18n/` folder next to the executable (or in `assets/i18n/`) is discovered
//! at startup, so adding a language requires no code changes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

/// A selectable language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lang {
    code: String,
    name: String,
}

impl Lang {
    pub fn new(code: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            name: name.into(),
        }
    }

    /// Short code used in `config.json` and dictionary file names.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Native language name shown in the selector.
    pub fn native_name(&self) -> &str {
        &self.name
    }

    /// Resolve a saved code against the currently available languages.
    pub fn from_code(code: &str) -> Lang {
        let code = code.trim().to_ascii_lowercase();
        if code.is_empty() {
            return Lang::new("es", "Español");
        }
        available_languages()
            .into_iter()
            .find(|lang| lang.code == code)
            .unwrap_or_else(|| Lang::new("en", "English"))
    }
}

/// Languages embedded in the executable.
pub const BUILTIN_LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("es", "Español"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("ko", "한국어"),
];

/// Directory of the running executable (falls back to the current directory).
fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Where dictionaries may live, in priority order.
fn dictionary_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        exe_dir().join("i18n"),
        exe_dir().join("assets").join("i18n"),
    ];
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("i18n"));
        dirs.push(cwd.join("assets").join("i18n"));
    }
    dirs
}

fn embedded_dictionary(code: &str) -> Option<&'static str> {
    match code {
        "es" => Some(include_str!("../assets/i18n/es.json")),
        "en" => Some(include_str!("../assets/i18n/en.json")),
        "fr" => Some(include_str!("../assets/i18n/fr.json")),
        "de" => Some(include_str!("../assets/i18n/de.json")),
        "ko" => Some(include_str!("../assets/i18n/ko.json")),
        _ => None,
    }
}

fn read_external_dictionary(code: &str) -> Option<String> {
    for dir in dictionary_dirs() {
        let path = dir.join(format!("{code}.json"));
        if let Ok(raw) = std::fs::read_to_string(&path) {
            return Some(raw);
        }
    }
    None
}

/// Languages available at startup: embedded ES/EN plus any external JSON.
/// The result is intended to be cached by the caller (the GUI selector reads it
/// once), not called every frame.
pub fn available_languages() -> Vec<Lang> {
    let mut langs: Vec<Lang> = BUILTIN_LANGUAGES
        .iter()
        .map(|(code, name)| Lang::new(*code, *name))
        .collect();
    let mut seen: Vec<String> = langs.iter().map(|l| l.code.clone()).collect();

    for dir in dictionary_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if stem.is_empty() || seen.contains(&stem) {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
                continue;
            };
            let code = value
                .get("_code")
                .and_then(serde_json::Value::as_str)
                .map(str::to_ascii_lowercase)
                .unwrap_or_else(|| stem.clone());
            if seen.contains(&code) {
                continue;
            }
            let name = value
                .get("_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&code)
                .to_string();
            seen.push(code.clone());
            langs.push(Lang::new(code, name));
        }
    }
    langs
}

/// Loaded dictionary for one language.
#[derive(Debug, Clone)]
pub struct Translator {
    lang: Lang,
    map: HashMap<String, String>,
}

impl Translator {
    fn load(lang: &Lang) -> Self {
        let raw = read_external_dictionary(lang.code())
            .or_else(|| embedded_dictionary(lang.code()).map(str::to_string));
        let mut map: HashMap<String, String> = raw
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default();
        map.remove("_code");
        map.remove("_name");
        Self {
            lang: lang.clone(),
            map,
        }
    }

    /// Translate a source string. Missing keys fall back to English, then to
    /// the Spanish source.
    pub fn t(&self, source: &str) -> String {
        if let Some(value) = self.map.get(source) {
            return value.clone();
        }
        if self.lang.code() != "en" {
            if let Some(value) = english_map().get(source) {
                return value.clone();
            }
        }
        source.to_string()
    }

    /// Translate a source string with `{0}`, `{1}`… placeholders.
    pub fn tf(&self, source: &str, args: &[&str]) -> String {
        let mut text = self.t(source);
        for (i, value) in args.iter().enumerate() {
            text = text.replace(&format!("{{{i}}}"), value);
        }
        text
    }
}

static TRANSLATOR: OnceLock<RwLock<Translator>> = OnceLock::new();
static ENGLISH: OnceLock<HashMap<String, String>> = OnceLock::new();

fn english_map() -> &'static HashMap<String, String> {
    ENGLISH.get_or_init(|| {
        let mut map: HashMap<String, String> =
            serde_json::from_str(include_str!("../assets/i18n/en.json")).unwrap_or_default();
        map.remove("_code");
        map.remove("_name");
        map
    })
}

fn cell() -> &'static RwLock<Translator> {
    TRANSLATOR.get_or_init(|| RwLock::new(Translator::load(&Lang::new("en", "English"))))
}

/// Set the active language (lazy initialization included).
pub fn set_lang(lang: Lang) {
    if let Ok(mut guard) = cell().write() {
        *guard = Translator::load(&lang);
    }
}

pub fn current_lang() -> Lang {
    cell()
        .read()
        .map(|t| t.lang.clone())
        .unwrap_or_else(|_| Lang::new("en", "English"))
}

/// Translate a source string in the active language.
pub fn tr(source: &str) -> String {
    cell()
        .read()
        .map(|t| t.t(source))
        .unwrap_or_else(|_| source.to_string())
}

/// Translate with positional placeholders.
pub fn trf(source: &str, args: &[&str]) -> String {
    cell()
        .read()
        .map(|t| t.tf(source, args))
        .unwrap_or_else(|_| source.to_string())
}

fn config_path() -> PathBuf {
    exe_dir().join("config.json")
}

/// Read the language saved by the GUI (or previous CLI run).
pub fn load_saved_lang() -> Lang {
    let Ok(raw) = std::fs::read_to_string(config_path()) else {
        return Lang::from_code("en");
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Lang::from_code("en");
    };
    let code = value
        .get("lang")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("en");
    Lang::from_code(code)
}

/// Persist the chosen language without touching other config keys.
pub fn save_saved_lang(lang: &Lang) {
    let mut value = std::fs::read_to_string(config_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "lang".to_string(),
            serde_json::Value::String(lang.code().to_string()),
        );
    }
    if let Ok(json) = serde_json::to_string_pretty(&value) {
        let _ = std::fs::write(config_path(), json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_dictionaries_parse_and_are_not_empty() {
        for (code, name) in BUILTIN_LANGUAGES {
            let translator = Translator::load(&Lang::new(*code, *name));
            assert!(
                !translator.map.is_empty(),
                "{code} dictionary is empty"
            );
        }
    }

    #[test]
    fn builtins_are_available() {
        let codes: Vec<String> = available_languages()
            .into_iter()
            .map(|lang| lang.code().to_string())
            .collect();
        for code in ["en", "es", "fr", "de", "ko"] {
            assert!(codes.iter().any(|c| c == code), "missing {code}");
        }
    }

    #[test]
    fn french_translates_a_known_label() {
        let translator = Translator::load(&Lang::new("fr", "Français"));
        assert_eq!(translator.t("Inicio"), "Accueil");
        assert_eq!(translator.t("AZUL"), "BLEU");
    }

    #[test]
    fn english_translates_a_known_label() {
        let translator = Translator::load(&Lang::new("en", "English"));
        assert_eq!(translator.t("Inicio"), "Home");
        assert_eq!(translator.t("AZUL"), "BLUE");
    }

    #[test]
    fn placeholders_are_replaced() {
        let translator = Translator::load(&Lang::new("en", "English"));
        assert_eq!(
            translator.tf("[{0} – {1}]  +{2}", &["00:30", "01:00", "120"]),
            "[00:30 – 01:00]  +120"
        );
    }

    #[test]
    fn unknown_language_falls_back_to_english_then_source() {
        let translator = Translator::load(&Lang::new("xx", "Test"));
        assert_eq!(translator.t("Inicio"), "Home");
        assert_eq!(translator.t("__missing_key__"), "__missing_key__");
    }
}
