//! Portable INI settings stored next to the executable.

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Settings {
    pub general: GeneralSettings,
    pub appearance: AppearanceSettings,
    pub monitor: MonitorSettings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "fields intentionally mirror the existing portable INI keys"
)]
pub struct GeneralSettings {
    pub autostart: bool,
    pub replace_alt_tab: bool,
    pub replace_win_tab: bool,
    pub typed_search: bool,
    pub release_alt_switches: bool,
    pub release_right_button_switches: bool,
    pub right_button_wheel_switching: bool,
    pub mouse_over_selection: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "fields intentionally mirror the existing portable INI keys"
)]
pub struct AppearanceSettings {
    pub icon: IconColor,
    pub theme: Theme,
    pub compact_list: bool,
    pub large_icons: bool,
    pub show_numbers: bool,
    pub show_app_names: bool,
    pub visible_borders: bool,
    pub preview: bool,
    pub full_desktop_preview: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IconColor {
    #[default]
    Azure,
    Copper,
    Ember,
    Indigo,
    Orchid,
    Rosewood,
    Vermilion,
    Violet,
}

impl IconColor {
    pub const ALL: [Self; 8] = [
        Self::Azure,
        Self::Copper,
        Self::Ember,
        Self::Indigo,
        Self::Orchid,
        Self::Rosewood,
        Self::Vermilion,
        Self::Violet,
    ];

    #[must_use]
    pub fn parse(value: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|color| color.as_ini_value().eq_ignore_ascii_case(value))
            .unwrap_or_default()
    }

    #[must_use]
    pub const fn as_ini_value(self) -> &'static str {
        match self {
            Self::Azure => "Azure",
            Self::Copper => "Copper",
            Self::Ember => "Ember",
            Self::Indigo => "Indigo",
            Self::Orchid => "Orchid",
            Self::Rosewood => "Rosewood",
            Self::Vermilion => "Vermilion",
            Self::Violet => "Violet",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Theme {
    #[default]
    Auto,
    Light,
    Dark,
}

impl Theme {
    #[must_use]
    pub fn parse(value: &str) -> Self {
        if value.eq_ignore_ascii_case("light") {
            Self::Light
        } else if value.eq_ignore_ascii_case("dark") {
            Self::Dark
        } else {
            Self::Auto
        }
    }

    #[must_use]
    pub const fn as_ini_value(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MonitorSettings {
    pub monitor_mode: String,
    pub use_current_monitor_filter: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            general: GeneralSettings {
                autostart: true,
                replace_alt_tab: true,
                replace_win_tab: true,
                typed_search: true,
                release_alt_switches: true,
                release_right_button_switches: true,
                right_button_wheel_switching: true,
                mouse_over_selection: true,
            },
            appearance: AppearanceSettings {
                icon: IconColor::Azure,
                theme: Theme::Auto,
                compact_list: true,
                large_icons: true,
                show_numbers: true,
                show_app_names: false,
                visible_borders: false,
                preview: true,
                full_desktop_preview: false,
            },
            monitor: MonitorSettings {
                monitor_mode: "CurrentByCursor".to_owned(),
                use_current_monitor_filter: false,
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Entry {
    section: String,
    key: String,
    value: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SettingsDocument {
    entries: Vec<Entry>,
}

impl SettingsDocument {
    #[must_use]
    pub fn parse(contents: &str) -> Self {
        let mut entries = Vec::new();
        let mut section = String::new();

        for raw_line in contents.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if let Some(value) = line
                .strip_prefix('[')
                .and_then(|value| value.strip_suffix(']'))
            {
                value.trim().clone_into(&mut section);
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            if key.is_empty() {
                continue;
            }
            let entry = Entry {
                section: section.clone(),
                key: key.to_owned(),
                value: value.trim().to_owned(),
            };
            if let Some(existing) = entries.iter_mut().find(|existing: &&mut Entry| {
                same_key(&existing.section, &entry.section) && same_key(&existing.key, &entry.key)
            }) {
                *existing = entry;
            } else {
                entries.push(entry);
            }
        }

        Self { entries }
    }

    #[must_use]
    pub fn settings(&self) -> Settings {
        let defaults = Settings::default();
        Settings {
            general: GeneralSettings {
                autostart: self.read_bool("General", "Autostart", defaults.general.autostart),
                replace_alt_tab: self.read_bool(
                    "General",
                    "ReplaceAltTab",
                    defaults.general.replace_alt_tab,
                ),
                replace_win_tab: self.read_bool(
                    "General",
                    "ReplaceWinTab",
                    defaults.general.replace_win_tab,
                ),
                typed_search: self.read_bool(
                    "General",
                    "TypedSearch",
                    defaults.general.typed_search,
                ),
                release_alt_switches: self.read_bool(
                    "General",
                    "ReleaseAltSwitches",
                    defaults.general.release_alt_switches,
                ),
                release_right_button_switches: self.read_bool(
                    "General",
                    "ReleaseRmbSwitches",
                    defaults.general.release_right_button_switches,
                ),
                right_button_wheel_switching: self.read_bool(
                    "General",
                    "RmbWheelSwitching",
                    defaults.general.right_button_wheel_switching,
                ),
                mouse_over_selection: self.read_bool(
                    "General",
                    "MouseOverSelection",
                    defaults.general.mouse_over_selection,
                ),
            },
            appearance: AppearanceSettings {
                icon: self
                    .find("Appearance", "Icon")
                    .map_or(defaults.appearance.icon, IconColor::parse),
                theme: self
                    .find("Appearance", "Theme")
                    .map_or(defaults.appearance.theme, Theme::parse),
                compact_list: self.read_bool(
                    "Appearance",
                    "CompactList",
                    defaults.appearance.compact_list,
                ),
                large_icons: self.read_bool(
                    "Appearance",
                    "LargeIcons",
                    defaults.appearance.large_icons,
                ),
                show_numbers: self.read_bool(
                    "Appearance",
                    "ShowNumbers",
                    defaults.appearance.show_numbers,
                ),
                show_app_names: self.read_bool(
                    "Appearance",
                    "ShowAppNames",
                    defaults.appearance.show_app_names,
                ),
                visible_borders: self.read_bool(
                    "Appearance",
                    "VisibleBorders",
                    defaults.appearance.visible_borders,
                ),
                preview: self.read_bool("Appearance", "Preview", defaults.appearance.preview),
                full_desktop_preview: self.read_bool(
                    "Appearance",
                    "FullDesktopPreview",
                    defaults.appearance.full_desktop_preview,
                ),
            },
            monitor: MonitorSettings {
                monitor_mode: self.read_string("Monitor", "Mode", &defaults.monitor.monitor_mode),
                use_current_monitor_filter: self.read_bool(
                    "Monitor",
                    "UseCurrentMonitorFilter",
                    defaults.monitor.use_current_monitor_filter,
                ),
            },
        }
    }

    #[must_use]
    pub fn render(&self, settings: &Settings) -> String {
        let mut lines = vec![
            "[General]".to_owned(),
            format!("Autostart={}", settings.general.autostart),
            format!("ReplaceAltTab={}", settings.general.replace_alt_tab),
            format!("ReplaceWinTab={}", settings.general.replace_win_tab),
            format!("TypedSearch={}", settings.general.typed_search),
            format!(
                "ReleaseAltSwitches={}",
                settings.general.release_alt_switches
            ),
            format!(
                "ReleaseRmbSwitches={}",
                settings.general.release_right_button_switches
            ),
            format!(
                "RmbWheelSwitching={}",
                settings.general.right_button_wheel_switching
            ),
            format!(
                "MouseOverSelection={}",
                settings.general.mouse_over_selection
            ),
            String::new(),
            "[Appearance]".to_owned(),
            format!("Icon={}", settings.appearance.icon.as_ini_value()),
            format!("Theme={}", settings.appearance.theme.as_ini_value()),
            format!("CompactList={}", settings.appearance.compact_list),
            format!("LargeIcons={}", settings.appearance.large_icons),
            format!("ShowNumbers={}", settings.appearance.show_numbers),
            format!("ShowAppNames={}", settings.appearance.show_app_names),
            format!("VisibleBorders={}", settings.appearance.visible_borders),
            format!("Preview={}", settings.appearance.preview),
            format!(
                "FullDesktopPreview={}",
                settings.appearance.full_desktop_preview
            ),
            String::new(),
            "[Monitor]".to_owned(),
            format!("Mode={}", settings.monitor.monitor_mode),
            format!(
                "UseCurrentMonitorFilter={}",
                settings.monitor.use_current_monitor_filter
            ),
        ];

        let mut current_section: Option<&str> = None;
        for entry in self.entries.iter().filter(|entry| !is_known(entry)) {
            if current_section.is_none_or(|section| !same_key(section, &entry.section)) {
                lines.push(String::new());
                if !entry.section.is_empty() {
                    lines.push(format!("[{}]", entry.section));
                }
                current_section = Some(&entry.section);
            }
            lines.push(format!("{}={}", entry.key, entry.value));
        }

        lines.join("\n") + "\n"
    }

    fn find(&self, section: &str, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| same_key(&entry.section, section) && same_key(&entry.key, key))
            .map(|entry| entry.value.as_str())
    }

    fn read_bool(&self, section: &str, key: &str, fallback: bool) -> bool {
        self.find(section, key)
            .and_then(parse_bool)
            .unwrap_or(fallback)
    }

    fn read_string(&self, section: &str, key: &str, fallback: &str) -> String {
        self.find(section, key)
            .filter(|value| !value.is_empty())
            .unwrap_or(fallback)
            .to_owned()
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    if value.eq_ignore_ascii_case("true") {
        Some(true)
    } else if value.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        None
    }
}

fn same_key(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn is_known(entry: &Entry) -> bool {
    const KNOWN: [(&str, &str); 19] = [
        ("General", "Autostart"),
        ("General", "ReplaceAltTab"),
        ("General", "ReplaceWinTab"),
        ("General", "TypedSearch"),
        ("General", "ReleaseAltSwitches"),
        ("General", "ReleaseRmbSwitches"),
        ("General", "RmbWheelSwitching"),
        ("General", "MouseOverSelection"),
        ("Appearance", "Icon"),
        ("Appearance", "Theme"),
        ("Appearance", "CompactList"),
        ("Appearance", "LargeIcons"),
        ("Appearance", "ShowNumbers"),
        ("Appearance", "ShowAppNames"),
        ("Appearance", "VisibleBorders"),
        ("Appearance", "Preview"),
        ("Appearance", "FullDesktopPreview"),
        ("Monitor", "Mode"),
        ("Monitor", "UseCurrentMonitorFilter"),
    ];
    KNOWN
        .iter()
        .any(|(section, key)| same_key(&entry.section, section) && same_key(&entry.key, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_values_use_the_established_product_defaults() {
        let settings = SettingsDocument::parse("").settings();

        assert_eq!(settings, Settings::default());
        assert_eq!(settings.appearance.icon, IconColor::Azure);
        assert!(settings.appearance.compact_list);
        assert!(!settings.appearance.show_app_names);
        assert!(!settings.appearance.visible_borders);
        assert!(!settings.appearance.full_desktop_preview);
        assert!(settings.general.typed_search);
    }

    #[test]
    fn explicitly_disabled_typed_search_remains_disabled() {
        let document = SettingsDocument::parse(
            "[General]\nTypedSearch=false\nFutureSetting=keep\n[Plugin]\nMode=Fast\n",
        );
        let settings = document.settings();

        assert!(!settings.general.typed_search);
        let rendered = document.render(&settings);
        assert!(rendered.contains("TypedSearch=false"));
        assert!(rendered.contains("[General]\nFutureSetting=keep"));
        assert!(rendered.contains("[Plugin]\nMode=Fast"));
    }

    #[test]
    fn known_values_are_case_insensitive_and_invalid_booleans_use_defaults() {
        let document = SettingsDocument::parse(
            "[general]\nreplacealttab=FALSE\nReplaceWinTab=not-a-bool\nTypedSearch=TRUE\n\
             [appearance]\nicon=Orchid\ntheme=Dark\nCompactList=false\nShowAppNames=true\n\
             VisibleBorders=false\nFullDesktopPreview=true\n",
        );
        let settings = document.settings();

        assert!(!settings.general.replace_alt_tab);
        assert!(settings.general.replace_win_tab);
        assert!(settings.general.typed_search);
        assert_eq!(settings.appearance.icon, IconColor::Orchid);
        assert_eq!(settings.appearance.theme, Theme::Dark);
        assert!(!settings.appearance.compact_list);
        assert!(settings.appearance.show_app_names);
        assert!(!settings.appearance.visible_borders);
        assert!(settings.appearance.full_desktop_preview);
    }

    #[test]
    fn rendering_preserves_unknown_values_but_rewrites_known_values() {
        let document = SettingsDocument::parse(
            "[General]\nReplaceAltTab=false\nTypedSearch=false\nFutureSetting=keep\n\
             [Plugin]\nMode=Fast\n",
        );
        let mut settings = document.settings();
        settings.general.replace_alt_tab = true;
        settings.general.typed_search = true;
        settings.appearance.icon = IconColor::Violet;
        settings.appearance.compact_list = false;
        settings.appearance.show_app_names = true;
        settings.appearance.visible_borders = false;
        let rendered = document.render(&settings);

        assert!(rendered.contains("ReplaceAltTab=true"));
        assert!(rendered.contains("TypedSearch=true"));
        assert!(rendered.contains("Icon=Violet"));
        assert!(rendered.contains("CompactList=false"));
        assert!(rendered.contains("ShowAppNames=true"));
        assert!(rendered.contains("VisibleBorders=false"));
        assert!(rendered.contains("[General]\nFutureSetting=keep"));
        assert!(rendered.contains("[Plugin]\nMode=Fast"));
        assert_eq!(rendered.matches("ReplaceAltTab=").count(), 1);
        assert_eq!(rendered.matches("TypedSearch=").count(), 1);
    }

    #[test]
    fn later_duplicate_value_wins_like_the_existing_store() {
        let document =
            SettingsDocument::parse("[General]\nReplaceAltTab=true\nReplaceAltTab=false\n");

        assert!(!document.settings().general.replace_alt_tab);
    }

    #[test]
    fn established_user_settings_round_trip_without_changes() {
        let contents = "[General]\nAutostart=true\nReplaceAltTab=true\nReplaceWinTab=true\n\
                        TypedSearch=true\nReleaseAltSwitches=true\nReleaseRmbSwitches=true\n\
                        RmbWheelSwitching=true\nMouseOverSelection=true\n\n[Appearance]\n\
                        Icon=Azure\nTheme=Auto\nCompactList=true\nLargeIcons=true\nShowNumbers=true\n\
                        ShowAppNames=false\nVisibleBorders=false\nPreview=true\n\
                        FullDesktopPreview=true\n\n[Monitor]\nMode=CurrentByCursor\n\
                        UseCurrentMonitorFilter=false\n";
        let document = SettingsDocument::parse(contents);

        assert_eq!(document.render(&document.settings()), contents);
    }

    #[test]
    fn theme_parsing_is_case_insensitive_and_invalid_values_use_auto() {
        for (value, expected) in [
            ("Auto", Theme::Auto),
            ("LIGHT", Theme::Light),
            ("dark", Theme::Dark),
            ("unsupported", Theme::Auto),
            ("", Theme::Auto),
        ] {
            let document = SettingsDocument::parse(&format!("[Appearance]\nTheme={value}\n"));

            assert_eq!(document.settings().appearance.theme, expected);
        }
    }

    #[test]
    fn icon_parsing_is_case_insensitive_and_invalid_values_use_azure() {
        for (value, expected) in [
            ("Azure", IconColor::Azure),
            ("COPPER", IconColor::Copper),
            ("ember", IconColor::Ember),
            ("Indigo", IconColor::Indigo),
            ("orchid", IconColor::Orchid),
            ("Rosewood", IconColor::Rosewood),
            ("vermilion", IconColor::Vermilion),
            ("Violet", IconColor::Violet),
            ("unsupported", IconColor::Azure),
            ("", IconColor::Azure),
        ] {
            let document = SettingsDocument::parse(&format!("[Appearance]\nIcon={value}\n"));

            assert_eq!(document.settings().appearance.icon, expected);
        }
    }

    #[test]
    fn supported_icons_are_persisted_with_canonical_names() {
        let document = SettingsDocument::parse("[Appearance]\nIcon=orchid\n");

        for icon in IconColor::ALL {
            let mut settings = document.settings();
            settings.appearance.icon = icon;

            assert!(
                document
                    .render(&settings)
                    .contains(&format!("Icon={}", icon.as_ini_value()))
            );
        }
    }

    #[test]
    fn supported_themes_are_persisted_with_canonical_names() {
        let document = SettingsDocument::parse("[Appearance]\nTheme=dark\n");

        for (theme, expected) in [
            (Theme::Auto, "Theme=Auto"),
            (Theme::Light, "Theme=Light"),
            (Theme::Dark, "Theme=Dark"),
        ] {
            let mut settings = document.settings();
            settings.appearance.theme = theme;

            assert!(document.render(&settings).contains(expected));
        }
    }
}
