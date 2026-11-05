# Interface translations

AltTabio embeds English, Russian, and Simplified Chinese UI text in the portable library.
`src/localization/messages.rs` keeps each English source message beside both translations.
UI code calls `tr` for a label or `message` for a template with named placeholders.
Window titles, application names, file paths, INI keys, and technical error details are not
translated. New translations must retain the same placeholder names and keyboard shortcuts.

The `[General]` INI key `Language` accepts `System`, `en`, `ru`, and `zh-Hans`.
Missing or unknown values select System. System examines the operating system's ordered
preferred UI languages and uses the first supported language, then English. Traditional
Chinese is unsupported and does not select Simplified Chinese implicitly. Explicit language
names in the picker always use their own language.

Windows previews the language within the Settings dialog. Saving commits it after settings
persistence succeeds; closing or cancelling restores the previous language. macOS follows its
existing immediate settings behavior. Native menus and update status use the selected language
without restarting the application.

To add a language, extend `Language`, its storage tag, native name, and preference matching;
add its column to the message catalog and translation dispatch; and verify its native layouts.
Catalog tests check unique sources, nonempty translations, and matching placeholders. Settings
tests check coverage, INI round trips, and English fallback. Windows tests exercise hidden
native language previews and measure translated labels at multiple DPIs. macOS changes also
need the macOS CI checks and app bundle build.
