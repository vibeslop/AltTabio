//! The Settings dialog's controls, their bindings to `Settings`, and their placement in pixels.

use crate::dialog_layout::{Point, Rect, Size, scale};
use crate::settings::{IconColor, Settings, Theme};
use std::iter;

const OPTION_COUNT: usize = 16;
const GENERAL_OPTION_COUNT: usize = 8;
const APPEARANCE_OPTION_COUNT: usize = 7;
// IDOK and IDCANCEL, which IsDialogMessageW sends for Enter and Esc.
const OK_ID: usize = 1;
const CANCEL_ID: usize = 2;
const OPTION_ID_BASE: usize = 100;
const CLIENT_WIDTH: i32 = 560;
const CLIENT_HEIGHT: i32 = 747;
const APPEARANCE_SELECTOR_WIDTH: i32 = 180;

/// Every child control of the dialog. The dialog creates, lays out, themes and paints its
/// controls by walking `Control::all` instead of naming each one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Control {
    Group(Group),
    /// The caption left of a drop-down.
    Label(Selector),
    Selector(Selector),
    Checkbox(SettingOption),
    Button(DialogButton),
}

impl Control {
    /// Every control in creation order, which is also the tab order.
    pub fn all() -> impl Iterator<Item = Self> {
        Group::ALL
            .into_iter()
            .flat_map(|group| {
                iter::once(Self::Group(group))
                    .chain(
                        group.selectors().iter().flat_map(|selector| {
                            [Self::Label(*selector), Self::Selector(*selector)]
                        }),
                    )
                    .chain(
                        SettingOption::ALL
                            .into_iter()
                            .filter(move |option| option.group() == group)
                            .map(Self::Checkbox),
                    )
            })
            .chain(DialogButton::ALL.map(Self::Button))
    }

    #[must_use]
    pub fn with_id(id: usize) -> Option<Self> {
        Self::all().find(|control| control.id() == Some(id))
    }

    /// Whether Tab moves the focus to the control.
    #[must_use]
    pub const fn is_tab_stop(self) -> bool {
        matches!(
            self,
            Self::Selector(_) | Self::Checkbox(_) | Self::Button(_)
        )
    }

    #[must_use]
    pub fn first_tab_stop() -> Option<Self> {
        Self::all().find(|control| control.is_tab_stop())
    }

    /// The command identifier of a control that reports to the dialog.
    #[must_use]
    pub const fn id(self) -> Option<usize> {
        match self {
            Self::Group(_) | Self::Label(_) => None,
            Self::Selector(selector) => Some(selector.control_id()),
            Self::Checkbox(option) => Some(option.control_id()),
            Self::Button(button) => Some(button.control_id()),
        }
    }

    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::Group(group) => group.title(),
            Self::Label(selector) => selector.label(),
            Self::Selector(_) => "",
            Self::Checkbox(option) => option.label(),
            Self::Button(button) => button.label(),
        }
    }

    /// The area the control shows.
    #[must_use]
    pub const fn rect(self, layout: &SettingsLayout) -> Rect {
        match self {
            Self::Group(group) => group.rect(layout),
            Self::Label(selector) => selector.label_rect(layout),
            Self::Selector(selector) => selector.rect(layout),
            Self::Checkbox(option) => option.rect(layout),
            Self::Button(button) => button.rect(layout),
        }
    }

    /// The bounds of the control's window. A combo box's window also spans its drop-down list,
    /// so it is taller than the field it shows.
    #[must_use]
    pub fn window_rect(self, layout: &SettingsLayout, dpi: u32) -> Rect {
        let rect = self.rect(layout);
        if let Self::Selector(_) = self {
            Rect {
                height: rect.height.saturating_add(scale(96, dpi)),
                ..rect
            }
        } else {
            rect
        }
    }
}

/// The group boxes that divide the dialog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Group {
    General,
    Appearance,
    Monitor,
}

impl Group {
    const ALL: [Self; 3] = [Self::General, Self::Appearance, Self::Monitor];

    #[must_use]
    const fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Monitor => "Monitor",
        }
    }

    /// The drop-downs at the top of the group, above its checkboxes.
    const fn selectors(self) -> &'static [Selector] {
        match self {
            Self::Appearance => &Selector::ALL,
            Self::General | Self::Monitor => &[],
        }
    }

    const fn rect(self, layout: &SettingsLayout) -> Rect {
        match self {
            Self::General => layout.general_group,
            Self::Appearance => layout.appearance_group,
            Self::Monitor => layout.monitor_group,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DialogButton {
    Ok,
    Cancel,
}

impl DialogButton {
    const ALL: [Self; 2] = [Self::Ok, Self::Cancel];

    #[must_use]
    const fn label(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Cancel => "Cancel",
        }
    }

    #[must_use]
    const fn control_id(self) -> usize {
        match self {
            Self::Ok => OK_ID,
            Self::Cancel => CANCEL_ID,
        }
    }

    const fn rect(self, layout: &SettingsLayout) -> Rect {
        match self {
            Self::Ok => layout.ok_button,
            Self::Cancel => layout.cancel_button,
        }
    }
}

// Declare each identity, field, label and placement together. Both directions of the
// binding are generated from the same field, so reads and writes cannot drift apart.
macro_rules! setting_options {
    ($( $name:ident: $group:ident, $row:literal, $section:ident.$field:ident, $label:literal; )+) => {
        /// The checkboxes, in tab order.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum SettingOption { $( $name, )+ }

        impl SettingOption {
            pub const ALL: [Self; OPTION_COUNT] = [$( Self::$name, )+];

            #[must_use]
            const fn label(self) -> &'static str {
                match self { $( Self::$name => $label, )+ }
            }

            #[must_use]
            const fn group(self) -> Group {
                match self { $( Self::$name => Group::$group, )+ }
            }

            #[must_use]
            const fn rect(self, layout: &SettingsLayout) -> Rect {
                let row = match self { $( Self::$name => $row, )+ };
                match self.group() {
                    Group::General => layout.general_options[row],
                    Group::Appearance => layout.appearance_options[row],
                    Group::Monitor => layout.monitor_option,
                }
            }

            #[must_use]
            pub const fn read(self, settings: &Settings) -> bool {
                match self { $( Self::$name => settings.$section.$field, )+ }
            }

            pub const fn write(self, settings: &mut Settings, value: bool) {
                match self { $( Self::$name => settings.$section.$field = value, )+ }
            }

            #[must_use]
            const fn control_id(self) -> usize { OPTION_ID_BASE + self as usize }
        }
    };
}

setting_options! {
    Autostart: General, 0, general.autostart, "Start AltTabio when I sign in";
    ReplaceAltTab: General, 1, general.replace_alt_tab, "Replace Alt+Tab";
    ReplaceWinTab: General, 2, general.replace_win_tab, "Replace Win+Tab";
    TypedSearch: General, 3, general.typed_search, "Enable typing to search tasks";
    ReleaseAltSwitches: General, 4, general.release_alt_switches, "Switch when Alt is released";
    ReleaseRightButtonSwitches: General, 5, general.release_right_button_switches, "Activate the selected task when the right mouse button is released";
    RightButtonWheelSwitching: General, 6, general.right_button_wheel_switching, "Use right mouse button + wheel switching";
    MouseOverSelection: General, 7, general.mouse_over_selection, "Select tasks when the mouse moves over them";
    CompactList: Appearance, 0, appearance.compact_list, "Use a compact task list";
    LargeIcons: Appearance, 1, appearance.large_icons, "Use large icons";
    ShowNumbers: Appearance, 2, appearance.show_numbers, "Show number shortcuts";
    ShowAppNames: Appearance, 3, appearance.show_app_names, "Show app names under titles";
    VisibleBorders: Appearance, 4, appearance.visible_borders, "Visible borders";
    Preview: Appearance, 5, appearance.preview, "Show a live preview";
    FullDesktopPreview: Appearance, 6, appearance.full_desktop_preview, "Show the window in its position on the desktop";
    CurrentMonitorFilter: Monitor, 0, monitor.use_current_monitor_filter, "Only show tasks from the current monitor";
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
    const ALL: [Self; 2] = [Self::Theme, Self::Icon];

    #[must_use]
    const fn label(self) -> &'static str {
        match self {
            Self::Theme => "Theme",
            Self::Icon => "Icon",
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

    const fn label_rect(self, layout: &SettingsLayout) -> Rect {
        match self {
            Self::Theme => layout.theme_label,
            Self::Icon => layout.icon_label,
        }
    }

    const fn rect(self, layout: &SettingsLayout) -> Rect {
        match self {
            Self::Theme => layout.theme_selector,
            Self::Icon => layout.icon_selector,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SettingsLayout {
    pub client: Size,
    general_group: Rect,
    general_options: [Rect; GENERAL_OPTION_COUNT],
    appearance_group: Rect,
    theme_label: Rect,
    theme_selector: Rect,
    icon_label: Rect,
    icon_selector: Rect,
    appearance_options: [Rect; APPEARANCE_OPTION_COUNT],
    monitor_group: Rect,
    monitor_option: Rect,
    ok_button: Rect,
    cancel_button: Rect,
}

impl SettingsLayout {
    #[must_use]
    pub fn for_dpi(dpi: u32) -> Self {
        let logical = Self::logical();
        Self {
            client: logical.client.scaled(dpi),
            general_group: logical.general_group.scaled(dpi),
            general_options: logical.general_options.map(|rect| rect.scaled(dpi)),
            appearance_group: logical.appearance_group.scaled(dpi),
            theme_label: logical.theme_label.scaled(dpi),
            theme_selector: logical.theme_selector.scaled(dpi),
            icon_label: logical.icon_label.scaled(dpi),
            icon_selector: logical.icon_selector.scaled(dpi),
            appearance_options: logical.appearance_options.map(|rect| rect.scaled(dpi)),
            monitor_group: logical.monitor_group.scaled(dpi),
            monitor_option: logical.monitor_option.scaled(dpi),
            ok_button: logical.ok_button.scaled(dpi),
            cancel_button: logical.cancel_button.scaled(dpi),
        }
    }

    #[must_use]
    const fn logical() -> Self {
        Self {
            client: Size::new(CLIENT_WIDTH, CLIENT_HEIGHT),
            general_group: Rect::new(20, 16, 520, 250),
            general_options: option_rows::<GENERAL_OPTION_COUNT>(38, 42, 484, 24, 27),
            appearance_group: Rect::new(20, 280, 520, 316),
            theme_label: Rect::new(38, 309, 64, 24),
            theme_selector: Rect::new(112, 304, APPEARANCE_SELECTOR_WIDTH, 30),
            icon_label: Rect::new(38, 347, 64, 24),
            icon_selector: Rect::new(112, 342, APPEARANCE_SELECTOR_WIDTH, 30),
            appearance_options: option_rows::<APPEARANCE_OPTION_COUNT>(38, 380, 484, 24, 27),
            monitor_group: Rect::new(20, 610, 520, 64),
            monitor_option: Rect::new(38, 635, 484, 24),
            ok_button: Rect::new(338, 695, 96, 32),
            cancel_button: Rect::new(444, 695, 96, 32),
        }
    }
}

const fn option_rows<const COUNT: usize>(
    x: i32,
    first_y: i32,
    width: i32,
    height: i32,
    step: i32,
) -> [Rect; COUNT] {
    let mut rows = [Rect::new(0, 0, 0, 0); COUNT];
    let mut index = 0;
    let mut y = first_y;
    while index < COUNT {
        rows[index] = Rect::new(x, y, width, height);
        y += step;
        index += 1;
    }
    rows
}

/// The three points of the tick drawn inside a checked box.
#[must_use]
pub const fn checkmark_points(square: Rect) -> [Point; 3] {
    let width = square.width;
    let height = square.height;
    [
        Point::new(
            square.x.saturating_add(width * 3 / 14),
            square.y.saturating_add(height * 7 / 14),
        ),
        Point::new(
            square.x.saturating_add(width * 6 / 14),
            square.y.saturating_add(height * 10 / 14),
        ),
        Point::new(
            square.x.saturating_add(width * 11 / 14),
            square.y.saturating_add(height * 4 / 14),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialog_layout::BASE_DPI;

    #[test]
    fn every_checkbox_reads_only_its_bound_setting() {
        let fields: [fn(&mut Settings) -> &mut bool; 16] = [
            |s| &mut s.general.autostart,
            |s| &mut s.general.replace_alt_tab,
            |s| &mut s.general.replace_win_tab,
            |s| &mut s.general.typed_search,
            |s| &mut s.general.release_alt_switches,
            |s| &mut s.general.release_right_button_switches,
            |s| &mut s.general.right_button_wheel_switching,
            |s| &mut s.general.mouse_over_selection,
            |s| &mut s.appearance.compact_list,
            |s| &mut s.appearance.large_icons,
            |s| &mut s.appearance.show_numbers,
            |s| &mut s.appearance.show_app_names,
            |s| &mut s.appearance.visible_borders,
            |s| &mut s.appearance.preview,
            |s| &mut s.appearance.full_desktop_preview,
            |s| &mut s.monitor.use_current_monitor_filter,
        ];
        let baseline = Settings::default();
        for (index, field) in fields.into_iter().enumerate() {
            let mut changed = baseline.clone();
            *field(&mut changed) = !*field(&mut changed);
            let mut expected = SettingOption::ALL.map(|option| option.read(&baseline));
            expected[index] = !expected[index];
            assert_eq!(
                SettingOption::ALL.map(|option| option.read(&changed)),
                expected,
                "option {index}"
            );
            let option = SettingOption::ALL[index];
            let mut roundtrip = baseline.clone();
            option.write(&mut roundtrip, option.read(&changed));
            assert_eq!(roundtrip, changed, "write for {option:?}");
            option.write(&mut roundtrip, option.read(&baseline));
            assert_eq!(roundtrip, baseline, "restore for {option:?}");
        }
    }

    #[test]
    fn named_options_preserve_unique_ids_and_layout_rows_at_each_dpi() {
        for dpi in [96, 120, 144, 192] {
            let layout = SettingsLayout::for_dpi(dpi);
            let rows = layout
                .general_options
                .into_iter()
                .chain(layout.appearance_options)
                .chain([layout.monitor_option]);
            for (index, (option, row)) in SettingOption::ALL.into_iter().zip(rows).enumerate() {
                assert_eq!(option.control_id(), OPTION_ID_BASE + index);
                assert_eq!(
                    option.rect(&layout),
                    row,
                    "layout for {option:?} at {dpi} DPI"
                );
            }
        }
    }

    #[test]
    fn visible_borders_option_tracks_the_appearance_setting() {
        let mut settings = Settings::default();

        assert_eq!(SettingOption::VisibleBorders.label(), "Visible borders");
        assert!(!SettingOption::VisibleBorders.read(&settings));

        settings.appearance.visible_borders = true;
        assert!(SettingOption::VisibleBorders.read(&settings));
    }

    #[test]
    fn typed_search_option_tracks_the_general_setting() {
        let mut settings = Settings::default();

        assert_eq!(
            SettingOption::TypedSearch.label(),
            "Enable typing to search tasks"
        );
        assert!(SettingOption::TypedSearch.read(&settings));

        settings.general.typed_search = false;
        assert!(!SettingOption::TypedSearch.read(&settings));
    }

    #[test]
    fn theme_entries_name_their_values_and_fall_back_to_auto() {
        let names = THEME_CHOICES.names().collect::<Vec<_>>();

        assert_eq!(names, ["Auto", "Light", "Dark"]);
        for (index, name) in names.into_iter().enumerate() {
            let theme = THEME_CHOICES.value_at(index);
            assert_eq!(Theme::parse(name), theme);
            assert_eq!(THEME_CHOICES.index_of(theme), index);
        }
        assert_eq!(THEME_CHOICES.value_at(usize::MAX), Theme::Auto);
    }

    #[test]
    fn icon_entries_follow_every_icon_color_in_order() {
        for (index, icon) in IconColor::ALL.into_iter().enumerate() {
            assert_eq!(ICON_CHOICES.index_of(icon), index);
            assert_eq!(ICON_CHOICES.value_at(index), icon);
        }
        assert!(
            ICON_CHOICES
                .names()
                .eq(IconColor::ALL.map(IconColor::as_ini_value))
        );
        assert_eq!(ICON_CHOICES.value_at(usize::MAX), IconColor::Azure);
    }

    #[test]
    fn checkmark_has_even_opposing_insets_inside_its_square() {
        let square = Rect::new(0, 0, 14, 14);

        let points = checkmark_points(square);

        assert_eq!(points[0], Point::new(3, 7));
        assert_eq!(points[1], Point::new(6, 10));
        assert_eq!(points[2], Point::new(11, 4));
        assert_eq!(points[0].x - square.x, square.right() - points[2].x);
        assert_eq!(points.iter().map(|point| point.y).min(), Some(square.y + 4));
        assert_eq!(
            points.iter().map(|point| point.y).max(),
            Some(square.bottom() - 4)
        );
    }

    #[test]
    fn logical_layout_keeps_every_control_inside_its_section_or_client() {
        let layout = SettingsLayout::logical();
        let client = Rect::new(0, 0, layout.client.width, layout.client.height);

        assert!(
            layout
                .general_options
                .into_iter()
                .all(|rect| layout.general_group.contains(rect))
        );
        assert!(layout.appearance_group.contains(layout.theme_label));
        assert!(layout.appearance_group.contains(layout.theme_selector));
        assert!(layout.appearance_group.contains(layout.icon_label));
        assert!(layout.appearance_group.contains(layout.icon_selector));
        assert!(
            layout
                .appearance_options
                .into_iter()
                .all(|rect| layout.appearance_group.contains(rect))
        );
        assert!(layout.monitor_group.contains(layout.monitor_option));
        assert!(client.contains(layout.general_group));
        assert!(client.contains(layout.appearance_group));
        assert!(client.contains(layout.monitor_group));
        assert!(client.contains(layout.ok_button));
        assert!(client.contains(layout.cancel_button));
        assert!(layout.ok_button.right() < layout.cancel_button.x);
        assert_eq!(layout.cancel_button.right(), layout.monitor_group.right());
    }

    #[test]
    fn appearance_selectors_share_one_aligned_column() {
        let layout = SettingsLayout::logical();

        assert_eq!(layout.theme_label.x, layout.icon_label.x);
        assert_eq!(layout.theme_selector.x, layout.icon_selector.x);
        assert_eq!(layout.theme_selector.width, layout.icon_selector.width);
        assert_eq!(layout.theme_selector.width, APPEARANCE_SELECTOR_WIDTH);
        assert!(layout.theme_selector.bottom() <= layout.icon_selector.y);
    }

    #[test]
    fn layout_scales_consistently_at_one_hundred_fifty_percent() {
        let normal = SettingsLayout::for_dpi(BASE_DPI);
        let scaled = SettingsLayout::for_dpi(144);

        assert_eq!(scaled.client.width, normal.client.width * 3 / 2);
        assert_eq!(scaled.client.height, scale(CLIENT_HEIGHT, 144));
        assert_eq!(scaled.general_group.x, normal.general_group.x * 3 / 2);
        assert_eq!(
            scaled.general_options[7].y,
            scale(normal.general_options[7].y, 144)
        );
        assert_eq!(
            scaled.theme_selector.width,
            normal.theme_selector.width * 3 / 2
        );
        assert_eq!(
            scaled.icon_selector.width,
            normal.icon_selector.width * 3 / 2
        );
        assert_eq!(
            scaled.cancel_button.right(),
            normal.cancel_button.right() * 3 / 2
        );
        assert_eq!(
            scaled.cancel_button.bottom(),
            scale(SettingsLayout::logical().cancel_button.bottom(), 144)
        );
    }

    #[test]
    fn control_table_lists_each_control_once_in_tab_order() {
        let controls = Control::all().collect::<Vec<_>>();

        assert_eq!(
            controls,
            [
                Control::Group(Group::General),
                Control::Checkbox(SettingOption::Autostart),
                Control::Checkbox(SettingOption::ReplaceAltTab),
                Control::Checkbox(SettingOption::ReplaceWinTab),
                Control::Checkbox(SettingOption::TypedSearch),
                Control::Checkbox(SettingOption::ReleaseAltSwitches),
                Control::Checkbox(SettingOption::ReleaseRightButtonSwitches),
                Control::Checkbox(SettingOption::RightButtonWheelSwitching),
                Control::Checkbox(SettingOption::MouseOverSelection),
                Control::Group(Group::Appearance),
                Control::Label(Selector::Theme),
                Control::Selector(Selector::Theme),
                Control::Label(Selector::Icon),
                Control::Selector(Selector::Icon),
                Control::Checkbox(SettingOption::CompactList),
                Control::Checkbox(SettingOption::LargeIcons),
                Control::Checkbox(SettingOption::ShowNumbers),
                Control::Checkbox(SettingOption::ShowAppNames),
                Control::Checkbox(SettingOption::VisibleBorders),
                Control::Checkbox(SettingOption::Preview),
                Control::Checkbox(SettingOption::FullDesktopPreview),
                Control::Group(Group::Monitor),
                Control::Checkbox(SettingOption::CurrentMonitorFilter),
                Control::Button(DialogButton::Ok),
                Control::Button(DialogButton::Cancel),
            ]
        );
        assert_eq!(
            Control::first_tab_stop(),
            Some(Control::Checkbox(SettingOption::Autostart))
        );
        for (index, control) in controls.iter().enumerate() {
            assert_eq!(
                controls.iter().filter(|other| *other == control).count(),
                1,
                "{control:?} at {index}"
            );
            if let Some(id) = control.id() {
                assert_eq!(Control::with_id(id), Some(*control));
            }
        }
        assert_eq!(
            Control::with_id(OK_ID),
            Some(Control::Button(DialogButton::Ok))
        );
        assert_eq!(
            Control::with_id(CANCEL_ID),
            Some(Control::Button(DialogButton::Cancel))
        );
    }

    #[test]
    fn controls_follow_the_group_box_that_contains_them() {
        let layout = SettingsLayout::logical();
        let mut group = None;
        for control in Control::all() {
            match control {
                Control::Group(next) => group = Some(next),
                Control::Button(_) => {}
                Control::Label(_) | Control::Selector(_) | Control::Checkbox(_) => {
                    let group = group.unwrap_or_else(|| panic!("{control:?} precedes every group"));
                    assert!(
                        group.rect(&layout).contains(control.rect(&layout)),
                        "{control:?} outside {group:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn only_drop_down_windows_extend_below_their_field() {
        let layout = SettingsLayout::for_dpi(144);

        for control in Control::all() {
            let field = control.rect(&layout);
            let window = control.window_rect(&layout, 144);
            let extension = if matches!(control, Control::Selector(_)) {
                144
            } else {
                0
            };
            assert_eq!(
                window,
                Rect {
                    height: field.height + extension,
                    ..field
                },
                "{control:?}"
            );
        }
    }
}
