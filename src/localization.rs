//! Shared UI language selection and embedded translations. INI keys and diagnostic details
//! remain stable across languages; English is the source and fallback for unknown messages.
mod messages;

use std::cell::Cell;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum Language {
    #[default]
    System,
    English,
    Russian,
    SimplifiedChinese,
}

impl Language {
    pub const ALL: [Self; 4] = [
        Self::System,
        Self::English,
        Self::Russian,
        Self::SimplifiedChinese,
    ];

    #[must_use]
    pub fn parse(value: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|language| language.as_ini_value().eq_ignore_ascii_case(value))
            .unwrap_or_default()
    }

    #[must_use]
    pub const fn as_ini_value(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::English => "en",
            Self::Russian => "ru",
            Self::SimplifiedChinese => "zh-Hans",
        }
    }

    #[must_use]
    pub fn native_name(self) -> &'static str {
        match self {
            Self::System => tr("System"),
            Self::English => "English",
            Self::Russian => "Русский",
            Self::SimplifiedChinese => "简体中文",
        }
    }

    /// The first supported language in the OS's ordered UI preferences, then English.
    /// Traditional Chinese is not silently substituted with Simplified Chinese.
    #[must_use]
    pub fn from_preferences(preferences: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        preferences
            .into_iter()
            .find_map(|tag| Self::from_tag(tag.as_ref()))
            .unwrap_or(Self::English)
    }

    fn from_tag(tag: &str) -> Option<Self> {
        let normalized = tag.replace('_', "-").to_ascii_lowercase();
        let parts: Vec<_> = normalized.split('-').collect();
        match *parts.first()? {
            "en" => Some(Self::English),
            "ru" => Some(Self::Russian),
            "zh" => {
                if parts.contains(&"hant") {
                    None
                } else if parts.contains(&"hans") {
                    Some(Self::SimplifiedChinese)
                } else if parts.iter().any(|part| matches!(*part, "tw" | "hk" | "mo")) {
                    None
                } else {
                    Some(Self::SimplifiedChinese)
                }
            }
            _ => None,
        }
    }

    #[must_use]
    pub fn translate(self, source: &str) -> &str {
        messages::translate(self, source)
    }
}

static SYSTEM: AtomicU8 = AtomicU8::new(Language::English as u8);
static SELECTED: AtomicU8 = AtomicU8::new(Language::System as u8);
thread_local! { static PREVIEW: Cell<Option<Language>> = const { Cell::new(None) }; }

fn decode(value: u8) -> Language {
    Language::ALL
        .get(usize::from(value))
        .copied()
        .unwrap_or_default()
}

/// Called outside native callbacks after querying the platform's UI preferences.
pub fn set_system_language(language: Language) {
    SYSTEM.store(
        if language == Language::System {
            Language::English
        } else {
            language
        } as u8,
        Ordering::Relaxed,
    );
}

/// Commits the language only after the settings change succeeds.
pub fn set_language(language: Language) {
    SELECTED.store(language as u8, Ordering::Relaxed);
}

#[must_use]
pub fn current_language() -> Language {
    let selected = PREVIEW
        .get()
        .unwrap_or_else(|| decode(SELECTED.load(Ordering::Relaxed)));
    if selected == Language::System {
        decode(SYSTEM.load(Ordering::Relaxed))
    } else {
        selected
    }
}

/// A modal Settings preview is local to its UI thread and is restored on every exit path.
pub struct PreviewLanguage {
    previous: Option<Language>,
    // A thread-local override must be restored on the thread that created it.
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl PreviewLanguage {
    #[must_use]
    pub fn new(language: Language) -> Self {
        Self {
            previous: PREVIEW.replace(Some(language)),
            _thread: std::marker::PhantomData,
        }
    }

    pub fn set(&self, language: Language) {
        PREVIEW.set(Some(language));
    }
}

impl Drop for PreviewLanguage {
    fn drop(&mut self) {
        PREVIEW.set(self.previous);
    }
}

#[must_use]
pub fn tr(source: &str) -> &str {
    current_language().translate(source)
}

/// Substitutes named placeholders once, without interpreting inserted window names or errors.
#[must_use]
pub fn message(source: &str, values: &[(&str, &str)]) -> String {
    interpolate(tr(source), values)
}

fn interpolate(mut template: &str, values: &[(&str, &str)]) -> String {
    let mut output = String::with_capacity(template.len());
    while let Some(start) = template.find('{') {
        output.push_str(&template[..start]);
        let remaining = &template[start + 1..];
        let Some(end) = remaining.find('}') else {
            output.push_str(&template[start..]);
            return output;
        };
        let name = &remaining[..end];
        if let Some((_, value)) = values.iter().find(|(key, _)| *key == name) {
            output.push_str(value);
        } else {
            output.push_str(&template[start..start + end + 2]);
        }
        template = &remaining[end + 1..];
    }
    output.push_str(template);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_ui_languages_match_language_and_script() {
        for tag in ["ru-RU", "ru-UA", "RU_ru"] {
            assert_eq!(Language::from_preferences([tag]), Language::Russian);
        }
        for tag in ["zh", "zh-CN", "zh-SG", "zh-Hans", "zh-Hans-HK"] {
            assert_eq!(
                Language::from_preferences([tag]),
                Language::SimplifiedChinese
            );
        }
        for tag in ["zh-Hant", "zh-Hant-CN", "zh-TW", "zh-HK", "zh-MO"] {
            assert_eq!(Language::from_preferences([tag, "ru"]), Language::Russian);
        }
        assert_eq!(
            Language::from_preferences(["de-DE", "ru", "en"]),
            Language::Russian
        );
        assert_eq!(
            Language::from_preferences(["en-GB", "ru"]),
            Language::English
        );
        assert_eq!(Language::from_preferences(["de-DE"]), Language::English);
    }

    #[test]
    fn preview_restores_previous_language_even_when_nested() {
        let initial = current_language();
        let outer = PreviewLanguage::new(Language::Russian);
        assert_eq!(tr("Settings"), "Настройки");
        {
            let _inner = PreviewLanguage::new(Language::SimplifiedChinese);
            assert_eq!(tr("Settings"), "设置");
        }
        assert_eq!(tr("Settings"), "Настройки");
        outer.set(Language::English);
        assert_eq!(tr("Settings"), "Settings");
        drop(outer);
        assert_eq!(current_language(), initial);
    }

    #[test]
    fn substitutions_preserve_user_content_and_unknown_messages() {
        assert_eq!(
            interpolate(
                "{app}: {error}",
                &[("app", "Notes {error}"), ("error", "ошибка")]
            ),
            "Notes {error}: ошибка"
        );
        assert_eq!(
            interpolate("{unknown} {unfinished", &[]),
            "{unknown} {unfinished"
        );
        assert_eq!(
            Language::Russian.translate("external diagnostic"),
            "external diagnostic"
        );
    }
}
