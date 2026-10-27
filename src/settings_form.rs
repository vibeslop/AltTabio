//! Settings controls and bindings shared by the native adapters.
mod layout;
pub use layout::{SettingsLayout, SettingsPages};

use crate::dialog_layout::{Rect, scale};
use crate::settings::{IconColor, Settings, Theme};

pub const SELECTOR_ITEM_HEIGHT: i32 = 32;
pub const SELECTOR_WIDTH: i32 = 160;
pub const MIN_CLIENT_WIDTH: i32 = 640;
pub const MIN_CLIENT_HEIGHT: i32 = 420;
const OPTION_COUNT: usize = 15;

/// A native control identity. Page controls live in a clipping, scrolling viewport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Control {
    Viewport,
    SidebarTitle,
    Navigation(Category),
    PageHeading,
    PageIntro,
    Section(Section),
    Label(Selector),
    Selector(Selector),
    Checkbox(SettingOption),
    AboutIdentity,
    AboutDescription,
    Button(DialogButton),
}

impl Control {
    pub fn all() -> impl Iterator<Item = Self> {
        [Self::SidebarTitle]
            .into_iter()
            .chain(Category::ALL.map(Self::Navigation))
            .chain([Self::PageHeading, Self::PageIntro, Self::Viewport])
            .chain(
                Category::ALL
                    .into_iter()
                    .flat_map(|category| category.controls().iter().copied()),
            )
            .chain(DialogButton::ALL.map(Self::Button))
    }

    #[must_use]
    pub fn with_id(id: usize) -> Option<Self> {
        Self::all().find(|control| control.id() == Some(id))
    }

    #[must_use]
    pub const fn is_tab_stop(self) -> bool {
        matches!(
            self,
            Self::Navigation(_) | Self::Selector(_) | Self::Checkbox(_) | Self::Button(_)
        )
    }

    #[must_use]
    pub const fn category(self) -> Option<Category> {
        match self {
            Self::Section(section) => Some(section.category()),
            Self::Label(_) | Self::Selector(_) => Some(Category::Appearance),
            Self::Checkbox(option) => Some(option.category()),
            Self::AboutIdentity | Self::AboutDescription => Some(Category::About),
            _ => None,
        }
    }

    #[must_use]
    pub const fn id(self) -> Option<usize> {
        match self {
            Self::Navigation(category) => Some(300 + category as usize),
            Self::Selector(selector) => Some(selector.control_id()),
            Self::Checkbox(option) => Some(option.control_id()),
            Self::Button(DialogButton::Ok) => Some(1),
            Self::Button(DialogButton::Cancel) => Some(2),
            _ => None,
        }
    }

    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::SidebarTitle => "Settings",
            Self::Navigation(category) => category.title(),
            Self::Section(section) => section.title(),
            Self::Label(selector) => selector.label(),
            Self::Checkbox(option) => option.label(),
            Self::AboutIdentity => concat!("AltTabio\nVersion ", env!("CARGO_PKG_VERSION")),
            Self::AboutDescription => "A window switcher for Windows and macOS.",
            Self::Button(DialogButton::Ok) => "Save changes",
            Self::Button(DialogButton::Cancel) => "Cancel",
            _ => "",
        }
    }

    #[must_use]
    pub fn rect(self, layout: &SettingsLayout) -> Rect {
        layout.control_rect(self)
    }

    /// A combo window includes the popup list below its visible field.
    #[must_use]
    pub fn window_rect(self, layout: &SettingsLayout, dpi: u32) -> Rect {
        let rect = self.rect(layout);
        if let Self::Selector(selector) = self {
            Rect {
                height: rect.height
                    + scale(SELECTOR_ITEM_HEIGHT, dpi)
                        * i32::try_from(selector.entries().len()).unwrap_or_default(),
                ..rect
            }
        } else {
            rect
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Category {
    #[default]
    General,
    Switching,
    Appearance,
    Display,
    About,
}

impl Category {
    pub const ALL: [Self; 5] = [
        Self::General,
        Self::Switching,
        Self::Appearance,
        Self::Display,
        Self::About,
    ];

    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Switching => "Switching",
            Self::Appearance => "Appearance",
            Self::Display => "Display",
            Self::About => "About",
        }
    }

    #[must_use]
    pub const fn intro(self) -> &'static str {
        match self {
            Self::General => "Startup and updates.",
            Self::Switching => "Choose how you move between windows.",
            Self::Appearance => "The look of your switcher.",
            Self::Display => "Windows across your monitors.",
            Self::About => "",
        }
    }

    #[must_use]
    pub const fn controls(self) -> &'static [Control] {
        use Control::{
            AboutDescription, AboutIdentity, Checkbox as C, Label, Section as S, Selector as D,
        };
        use SettingOption::{
            AutoUpdate, Autostart, CompactList, CurrentMonitorFilter, FullDesktopPreview,
            LargeIcons, MouseOverSelection, Preview, ReleaseAltSwitches, ReplaceAltTab,
            ReplaceWinTab, ShowAppNames, ShowNumbers, TypedSearch, VisibleBorders,
        };
        match self {
            Self::General => &[C(Autostart), C(AutoUpdate)],
            Self::Switching => &[
                S(Section::Keyboard),
                C(ReplaceAltTab),
                C(ReplaceWinTab),
                C(TypedSearch),
                C(ReleaseAltSwitches),
                S(Section::Mouse),
                C(MouseOverSelection),
            ],
            Self::Appearance => &[
                Label(Selector::Theme),
                D(Selector::Theme),
                Label(Selector::Icon),
                D(Selector::Icon),
                S(Section::WindowList),
                C(CompactList),
                C(LargeIcons),
                C(ShowNumbers),
                C(ShowAppNames),
                C(VisibleBorders),
                S(Section::Preview),
                C(Preview),
                C(FullDesktopPreview),
            ],
            Self::Display => &[C(CurrentMonitorFilter)],
            Self::About => &[AboutIdentity, AboutDescription],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Section {
    Keyboard,
    Mouse,
    WindowList,
    Preview,
}

impl Section {
    #[must_use]
    pub const fn category(self) -> Category {
        match self {
            Self::Keyboard | Self::Mouse => Category::Switching,
            Self::WindowList | Self::Preview => Category::Appearance,
        }
    }

    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Keyboard => "Keyboard",
            Self::Mouse => "Mouse",
            Self::WindowList => "Window list",
            Self::Preview => "Preview",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DialogButton {
    Ok,
    Cancel,
}
impl DialogButton {
    const ALL: [Self; 2] = [Self::Cancel, Self::Ok];
}

// Keep option identities stable while placing them in the new categories.
macro_rules! setting_options {
    ($( $name:ident: $category:ident, $section:ident.$field:ident, $label:literal; )+) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum SettingOption { $( $name, )+ }
        impl SettingOption {
            pub const ALL: [Self; OPTION_COUNT] = [$( Self::$name, )+];
            #[must_use]
            pub const fn label(self) -> &'static str { match self { $(Self::$name => $label,)+ } }
            #[must_use]
            pub const fn category(self) -> Category { match self { $(Self::$name => Category::$category,)+ } }
            #[must_use]
            pub const fn read(self, settings: &Settings) -> bool { match self { $(Self::$name => settings.$section.$field,)+ } }
            pub const fn write(self, settings: &mut Settings, value: bool) { match self { $(Self::$name => settings.$section.$field = value,)+ } }
            const fn control_id(self) -> usize { 100 + self as usize }
        }
    };
}

setting_options! {
    Autostart: General, general.autostart, "Start at sign-in";
    ReplaceAltTab: Switching, general.replace_alt_tab, "Replace Alt+Tab";
    ReplaceWinTab: Switching, general.replace_win_tab, "Replace Win+Tab";
    TypedSearch: Switching, general.typed_search, "Type to search";
    ReleaseAltSwitches: Switching, general.release_alt_switches, "Switch when Alt is released";
    MouseOverSelection: Switching, general.mouse_over_selection, "Select on hover";
    AutoUpdate: General, general.auto_update, "Automatic updates";
    CompactList: Appearance, appearance.compact_list, "Compact list";
    LargeIcons: Appearance, appearance.large_icons, "Large icons";
    ShowNumbers: Appearance, appearance.show_numbers, "Number shortcuts";
    ShowAppNames: Appearance, appearance.show_app_names, "App names under titles";
    VisibleBorders: Appearance, appearance.visible_borders, "Visible borders";
    Preview: Appearance, appearance.preview, "Live window preview";
    FullDesktopPreview: Appearance, appearance.full_desktop_preview, "Preview in desktop position";
    CurrentMonitorFilter: Display, monitor.use_current_monitor_filter, "Only show windows on the current monitor";
}

impl SettingOption {
    #[must_use]
    pub const fn help(self) -> &'static str {
        match self {
            Self::AutoUpdate => "Check for and install updates automatically.",
            Self::TypedSearch => "Find a window by typing while the switcher is open.",
            Self::MouseOverSelection => "Select a window when the pointer moves over it.",
            Self::FullDesktopPreview => "Show the window where it sits on the desktop.",
            _ => "",
        }
    }
}

/// One drop-down's values in list order, each shown by its INI name.
pub struct Choices<T: 'static> {
    values: &'static [T],
    name: fn(T) -> &'static str,
}

impl<T: Copy + Default + PartialEq> Choices<T> {
    fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.values.iter().map(|value| (self.name)(*value))
    }

    #[must_use]
    fn index_of(&self, value: T) -> usize {
        self.values
            .iter()
            .position(|candidate| *candidate == value)
            .unwrap_or_default()
    }

    /// The value at `index`, or the default when the list has no such entry.
    #[must_use]
    pub fn value_at(&self, index: usize) -> T {
        self.values.get(index).copied().unwrap_or_default()
    }
}

pub const THEME_CHOICES: Choices<Theme> = Choices {
    values: &[Theme::Auto, Theme::Light, Theme::Dark],
    name: Theme::as_ini_value,
};
pub const ICON_CHOICES: Choices<IconColor> = Choices {
    values: &IconColor::ALL,
    name: IconColor::as_ini_value,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Selector {
    Theme,
    Icon,
}

impl Selector {
    #[must_use]
    const fn label(self) -> &'static str {
        match self {
            Self::Theme => "Theme",
            Self::Icon => "App icon",
        }
    }

    #[must_use]
    const fn control_id(self) -> usize {
        match self {
            Self::Theme => 200,
            Self::Icon => 201,
        }
    }

    #[must_use]
    pub fn entries(self) -> Vec<&'static str> {
        match self {
            Self::Theme => THEME_CHOICES.names().collect(),
            Self::Icon => ICON_CHOICES.names().collect(),
        }
    }

    /// The entry that shows the current value in `settings`.
    #[must_use]
    pub fn index(self, settings: &Settings) -> usize {
        match self {
            Self::Theme => THEME_CHOICES.index_of(settings.appearance.theme),
            Self::Icon => ICON_CHOICES.index_of(settings.appearance.icon),
        }
    }
}

#[cfg(test)]
mod tests;
