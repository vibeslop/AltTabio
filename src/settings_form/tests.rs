use super::*;
use crate::dialog_layout::{Size, scale};
#[test]
fn every_checkbox_reads_only_its_bound_setting() {
    let fields: [fn(&mut Settings) -> &mut bool; 15] = [
        |s| &mut s.general.autostart,
        |s| &mut s.general.replace_alt_tab,
        |s| &mut s.general.replace_win_tab,
        |s| &mut s.general.typed_search,
        |s| &mut s.general.release_alt_switches,
        |s| &mut s.general.mouse_over_selection,
        |s| &mut s.general.auto_update,
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
fn category_pages_keep_all_bindings_once_with_unique_native_ids() {
    let controls = Control::all().collect::<Vec<_>>();
    for &control in &controls {
        assert_eq!(
            controls.iter().filter(|other| **other == control).count(),
            1,
            "{control:?}"
        );
        if let Some(id) = control.id() {
            assert_eq!(Control::with_id(id), Some(control));
        }
    }
    for option in SettingOption::ALL {
        assert_eq!(
            Control::Checkbox(option).category(),
            Some(option.category())
        );
        assert_eq!(
            controls
                .iter()
                .filter(|c| **c == Control::Checkbox(option))
                .count(),
            1
        );
    }
    assert_eq!(
        SettingOption::CurrentMonitorFilter.label(),
        "Only show windows on the current monitor"
    );
}

#[test]
fn layouts_keep_rows_clear_of_switches_and_footer_at_supported_sizes_and_dpis() {
    for dpi in [96, 120, 144, 192] {
        for size in [
            Size::new(728, 616),
            Size::new(640, 420),
            Size::new(1000, 800),
        ] {
            for category in Category::ALL {
                let layout = SettingsLayout::for_client(size.scaled(dpi), dpi, category);
                let client = Rect::new(0, 0, layout.client.width, layout.client.height);
                for control in [
                    Control::SidebarTitle,
                    Control::PageHeading,
                    Control::PageIntro,
                    Control::Navigation(category),
                    Control::Viewport,
                    Control::Button(DialogButton::Ok),
                    Control::Button(DialogButton::Cancel),
                ] {
                    assert!(
                        client.contains(control.rect(&layout)),
                        "{control:?}, {size:?}, {dpi}"
                    );
                }
                assert_eq!(layout.viewport.bottom(), layout.footer.y);
                assert!(
                    Control::Button(DialogButton::Ok).rect(&layout).right()
                        < Control::Button(DialogButton::Cancel).rect(&layout).x
                );
                let mut previous_bottom = 0;
                for &control in category.controls() {
                    let row = control.rect(&layout);
                    assert!(
                        row.x >= 0 && row.right() <= layout.viewport.width - scale(17, dpi),
                        "{control:?}"
                    );
                    if !matches!(control, Control::Label(_) | Control::Selector(_)) {
                        assert!(row.y >= previous_bottom, "overlap: {control:?}");
                        previous_bottom = row.bottom();
                    }
                }
            }
        }
    }
}

#[test]
fn navigation_restores_each_page_scroll_and_clamps_after_resize() {
    let mut pages = SettingsPages::default();
    let layout = SettingsLayout::for_dpi(96);
    pages.select(Category::Appearance);
    pages.scroll_to(i32::MAX, &layout);
    let appearance = pages.offset();
    assert!(appearance > 0);
    pages.select(Category::Switching);
    assert_eq!(pages.offset(), 0);
    pages.scroll_to(30, &layout);
    pages.select(Category::Appearance);
    assert_eq!(pages.offset(), appearance);
    let larger = SettingsLayout::for_client(Size::new(728, 1000), 96, Category::Appearance);
    pages.scroll_to(pages.offset(), &larger);
    assert_eq!(pages.offset(), 0);
}

#[test]
fn keyboard_focus_reveals_the_last_row_without_moving_the_heading() {
    let mut pages = SettingsPages::default();
    pages.select(Category::Appearance);
    let layout = SettingsLayout::for_dpi(96);
    let header = Control::PageHeading.rect(&layout);
    let row = Control::Checkbox(SettingOption::FullDesktopPreview).rect(&layout);
    pages.reveal(row, &layout);
    assert!(pages.offset() > 0);
    assert!(row.bottom() - pages.offset() <= layout.viewport.height);
    assert_eq!(Control::PageHeading.rect(&layout), header);
    pages.reveal(Control::Selector(Selector::Theme).rect(&layout), &layout);
    assert!(pages.offset() <= 11);
}

#[test]
fn high_resolution_wheel_and_dpi_changes_preserve_position() {
    let mut pages = SettingsPages::default();
    pages.select(Category::Appearance);
    let layout = SettingsLayout::for_dpi(96);
    for _ in 0..4 {
        pages.wheel(-30, 45, &layout);
    }
    assert_eq!(pages.offset(), 45);
    pages.rescale(96, 144);
    assert_eq!(pages.offset(), 67);
    pages.select(Category::General);
    pages.wheel(-120, 45, &layout);
    assert_eq!(pages.offset(), 0);
}
