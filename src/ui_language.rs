//! Platform UI-language preferences, queried at startup outside input callbacks.
use alttabio::localization::{Language, set_system_language};

pub(crate) fn initialize_system() {
    set_system_language(preferred_language());
}

#[cfg(windows)]
fn preferred_language() -> Language {
    match windows_preferences() {
        Ok(preferences) => Language::from_preferences(preferences),
        Err(error) => {
            eprintln!("Could not read preferred UI languages; using English: {error}");
            Language::English
        }
    }
}

#[cfg(windows)]
fn windows_preferences() -> windows::core::Result<Vec<String>> {
    use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    use windows::core::{Error, HRESULT, PWSTR};
    let mut count = 0;
    let mut size = 0;
    // SAFETY: this first call queries the required size; both scalar outputs are writable.
    unsafe {
        GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &raw mut count, None, &raw mut size)?;
    }
    if size == 0 || size > 65_536 {
        return Err(Error::from_hresult(HRESULT(0x8007_0057_u32.cast_signed())));
    }
    let mut buffer = vec![0; usize::try_from(size).unwrap_or_default()];
    // SAFETY: the UTF-16 buffer has exactly the capacity reported by the size query. A
    // concurrent preference change that needs more space returns an error, never a short write.
    unsafe {
        GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &raw mut count,
            Some(PWSTR(buffer.as_mut_ptr())),
            &raw mut size,
        )?;
    }
    Ok(buffer
        .split(|unit| *unit == 0)
        .take_while(|tag| !tag.is_empty())
        .map(String::from_utf16_lossy)
        .collect())
}

#[cfg(target_os = "macos")]
fn preferred_language() -> Language {
    let preferences = objc2_foundation::NSLocale::preferredLanguages();
    Language::from_preferences(preferences.iter().map(|language| language.to_string()))
}
