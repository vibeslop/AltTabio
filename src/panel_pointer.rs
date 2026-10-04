//! The pointer on the macOS switcher panel. The adapter resolves each event to a `Hit` against
//! the last frame drawn and applies the `Response`; the rest on a tile is a timer it runs.

use crate::app_switcher::Action;
use crate::close_button::CloseButtonVisualState;
use crate::panel_layout::{Hit, Shown};
use crate::window_command::WindowCommand;

/// How long the pointer rests on an app's tile before the app is selected, so a pointer that
/// crosses the strip on its way to a window does not change the app underneath it.
pub const TILE_DWELL_SECONDS: f64 = 0.08;
/// How far the pointer travels after the panel appears before hovering selects anything.
const HOVER_ARM_DISTANCE: f64 = 8.0;

/// What a pointer event asks of the switcher, applied in field order.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Response {
    pub dwell: Dwell,
    /// The app to select, by index, and its window, or its last-used one.
    pub select: Option<(usize, Option<usize>)>,
    pub action: Option<Action>,
    /// The close button draws differently.
    pub redraw: bool,
    pub menu: Option<MenuFor>,
}

/// The rest on a tile that selects its app.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Dwell {
    #[default]
    Keep,
    Cancel,
    /// Cancels any rest and calls `dwell_elapsed` with this app after `TILE_DWELL_SECONDS`.
    Start(usize),
}

/// What a right click opens the command menu for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuFor {
    Window,
    App,
}

#[derive(Debug, Default)]
pub struct Pointer {
    close_hovered: bool,
    close_pressed: bool,
    // The tile or row a click started on; the switch happens when it ends there too.
    pressed: Option<Hit>,
    // Where the pointer was when the panel appeared. Hovering selects nothing until the pointer
    // has moved away from here, so a nudge of the trackpad while ⌘ is down cannot change the
    // window the release switches to.
    origin: Option<(f64, f64)>,
    // The tile under the pointer, as an app index.
    hovered_tile: Option<usize>,
}

impl Pointer {
    /// Holds hovering back until the pointer leaves `location`, in screen points.
    pub fn panel_shown(&mut self, location: (f64, f64)) {
        self.origin = Some(location);
    }

    #[must_use]
    pub const fn close_state(&self) -> CloseButtonVisualState {
        if self.close_pressed {
            CloseButtonVisualState::Pressed
        } else if self.close_hovered {
            CloseButtonVisualState::Hovered
        } else {
            CloseButtonVisualState::Normal
        }
    }

    /// The pointer moved onto `hit` of `shown`, at `location` in screen points, while the app at
    /// `selected_app` was selected.
    pub fn moved(
        &mut self,
        hit: Option<Hit>,
        location: (f64, f64),
        shown: &Shown,
        selected_app: Option<usize>,
    ) -> Response {
        let hovered = matches!(hit, Some(Hit::CloseButton(_)));
        let close_changed = hovered != self.close_hovered;
        self.close_hovered = hovered;
        if let Some(origin) = self.origin {
            if (location.0 - origin.0).hypot(location.1 - origin.1) < HOVER_ARM_DISTANCE {
                return Response {
                    redraw: close_changed,
                    ..Response::default()
                };
            }
            self.origin = None;
        }
        let mut response = Response::default();
        let tile = match hit {
            Some(Hit::Tile(slot)) => Some(shown.tile_start + slot),
            _ => None,
        };
        if tile != self.hovered_tile {
            self.hovered_tile = tile;
            response.dwell = match tile {
                Some(app) if Some(app) != selected_app => Dwell::Start(app),
                _ => Dwell::Cancel,
            };
        }
        match hit {
            Some(Hit::Row(row)) if !self.close_pressed && shown.selected_row != Some(row) => {
                response.select = Some((
                    selected_app.unwrap_or_default(),
                    Some(shown.row_start + row),
                ));
            }
            _ => response.redraw = close_changed,
        }
        response
    }

    /// The rest that `Dwell::Start(app)` began is over.
    #[must_use]
    pub fn dwell_elapsed(&self, app: usize) -> Response {
        Response {
            select: (self.hovered_tile == Some(app)).then_some((app, None)),
            ..Response::default()
        }
    }

    pub fn pressed(
        &mut self,
        hit: Option<Hit>,
        shown: &Shown,
        selected_app: Option<usize>,
    ) -> Response {
        match hit {
            Some(Hit::CloseButton(_)) => {
                self.close_pressed = true;
                Response {
                    redraw: true,
                    ..Response::default()
                }
            }
            Some(Hit::Tile(slot)) => {
                self.pressed = hit;
                Response {
                    dwell: Dwell::Cancel,
                    select: Some((shown.tile_start + slot, None)),
                    ..Response::default()
                }
            }
            Some(Hit::Row(row)) => {
                self.pressed = hit;
                Response {
                    select: Some((
                        selected_app.unwrap_or_default(),
                        Some(shown.row_start + row),
                    )),
                    ..Response::default()
                }
            }
            None => Response::default(),
        }
    }

    pub fn released(&mut self, hit: Option<Hit>) -> Response {
        let pressed = self.pressed.take();
        if self.close_pressed {
            self.close_pressed = false;
            Response {
                action: matches!(hit, Some(Hit::CloseButton(_)))
                    .then_some(Action::Command(WindowCommand::Close)),
                redraw: true,
                ..Response::default()
            }
        } else if hit.is_some() && hit == pressed {
            Response {
                action: Some(Action::Activate),
                ..Response::default()
            }
        } else {
            Response::default()
        }
    }

    /// Selects what a right click landed on, for the command menu to act on.
    #[must_use]
    pub fn right_pressed(
        &self,
        hit: Option<Hit>,
        shown: &Shown,
        selected_app: Option<usize>,
    ) -> Response {
        match hit {
            Some(Hit::Row(row) | Hit::CloseButton(row)) => Response {
                select: Some((
                    selected_app.unwrap_or_default(),
                    Some(shown.row_start + row),
                )),
                menu: Some(MenuFor::Window),
                ..Response::default()
            },
            Some(Hit::Tile(slot)) => Response {
                dwell: Dwell::Cancel,
                select: Some((shown.tile_start + slot, None)),
                menu: Some(MenuFor::App),
                ..Response::default()
            },
            None => Response::default(),
        }
    }

    pub fn exited(&mut self) -> Response {
        self.hovered_tile = None;
        Response {
            dwell: Dwell::Cancel,
            redraw: std::mem::take(&mut self.close_hovered),
            ..Response::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel_layout::Layout;

    const AT_REST: (f64, f64) = (100.0, 100.0);
    const AWAY: (f64, f64) = (100.0, 100.0 + HOVER_ARM_DISTANCE);

    fn shown(tile_start: usize, row_start: usize, selected_row: Option<usize>) -> Shown {
        Shown {
            layout: Layout::new(8, 8, false, (1512.0, 900.0)),
            app: None,
            tile_start,
            tiles: 4,
            row_start,
            rows: 4,
            selected_row,
        }
    }

    fn select(app: usize, window: Option<usize>) -> Response {
        Response {
            select: Some((app, window)),
            ..Response::default()
        }
    }

    #[test]
    fn hovering_waits_until_the_pointer_leaves_where_the_panel_appeared() {
        let mut pointer = Pointer::default();
        let frame = shown(0, 2, Some(0));
        pointer.panel_shown(AT_REST);

        let nudged = (AT_REST.0 + 3.0, AT_REST.1 + 3.0);
        assert_eq!(
            pointer.moved(Some(Hit::Row(1)), nudged, &frame, Some(0)),
            Response::default()
        );
        assert_eq!(
            pointer.moved(Some(Hit::Row(1)), AWAY, &frame, Some(0)),
            select(0, Some(3))
        );
        // Once armed, it stays armed back at the starting point.
        assert_eq!(
            pointer.moved(Some(Hit::Row(2)), AT_REST, &frame, Some(0)),
            select(0, Some(4))
        );
    }

    #[test]
    fn the_close_button_lights_up_before_hovering_arms() {
        let mut pointer = Pointer::default();
        pointer.panel_shown(AT_REST);

        let response = pointer.moved(
            Some(Hit::CloseButton(0)),
            AT_REST,
            &shown(0, 0, Some(0)),
            Some(0),
        );

        assert!(response.redraw);
        assert_eq!(pointer.close_state(), CloseButtonVisualState::Hovered);
    }

    #[test]
    fn hovering_the_selected_row_or_its_close_button_selects_nothing() {
        let mut pointer = Pointer::default();
        let frame = shown(0, 0, Some(1));

        assert_eq!(
            pointer.moved(Some(Hit::Row(1)), AWAY, &frame, Some(0)),
            Response::default()
        );
        assert_eq!(
            pointer.moved(Some(Hit::CloseButton(1)), AWAY, &frame, Some(0)),
            Response {
                redraw: true,
                ..Response::default()
            }
        );
        // Moving off the close button onto another row selects it; the selection redraws.
        assert_eq!(
            pointer.moved(Some(Hit::Row(2)), AWAY, &frame, Some(0)),
            select(0, Some(2))
        );
        assert_eq!(pointer.close_state(), CloseButtonVisualState::Normal);
    }

    #[test]
    fn resting_on_a_tile_selects_its_app() {
        let mut pointer = Pointer::default();
        let frame = shown(2, 0, None);

        let response = pointer.moved(Some(Hit::Tile(1)), AWAY, &frame, Some(0));
        assert_eq!(response.dwell, Dwell::Start(3));
        assert_eq!(response.select, None);
        assert_eq!(
            pointer.moved(Some(Hit::Tile(1)), AWAY, &frame, Some(0)),
            Response::default()
        );
        assert_eq!(pointer.dwell_elapsed(3), select(3, None));
    }

    #[test]
    fn leaving_a_tile_drops_its_rest() {
        let mut pointer = Pointer::default();
        let frame = shown(0, 0, None);
        let _ = pointer.moved(Some(Hit::Tile(1)), AWAY, &frame, Some(0));

        assert_eq!(
            pointer.moved(None, AWAY, &frame, Some(0)).dwell,
            Dwell::Cancel
        );
        assert_eq!(pointer.dwell_elapsed(1), Response::default());

        let _ = pointer.moved(Some(Hit::Tile(1)), AWAY, &frame, Some(0));
        assert_eq!(pointer.exited().dwell, Dwell::Cancel);
        assert_eq!(pointer.dwell_elapsed(1), Response::default());
    }

    #[test]
    fn the_selected_apps_tile_needs_no_rest() {
        let mut pointer = Pointer::default();
        let frame = shown(0, 0, None);
        let _ = pointer.moved(Some(Hit::Tile(2)), AWAY, &frame, Some(1));

        assert_eq!(
            pointer
                .moved(Some(Hit::Tile(1)), AWAY, &frame, Some(1))
                .dwell,
            Dwell::Cancel
        );
    }

    #[test]
    fn a_click_switches_when_it_ends_where_it_started() {
        let mut pointer = Pointer::default();
        let frame = shown(1, 2, Some(0));

        assert_eq!(
            pointer.pressed(Some(Hit::Tile(0)), &frame, Some(3)),
            Response {
                dwell: Dwell::Cancel,
                select: Some((1, None)),
                ..Response::default()
            }
        );
        assert_eq!(
            pointer.released(Some(Hit::Tile(0))).action,
            Some(Action::Activate)
        );

        assert_eq!(
            pointer.pressed(Some(Hit::Row(1)), &frame, Some(3)),
            select(3, Some(3))
        );
        assert_eq!(pointer.released(Some(Hit::Row(2))), Response::default());
        // The press is spent; a release alone switches nothing.
        assert_eq!(pointer.released(Some(Hit::Row(1))), Response::default());
        assert_eq!(pointer.pressed(None, &frame, Some(3)), Response::default());
        assert_eq!(pointer.released(None), Response::default());
    }

    #[test]
    fn the_close_button_closes_when_the_press_ends_on_it() {
        let mut pointer = Pointer::default();
        let frame = shown(0, 0, Some(0));

        assert!(
            pointer
                .pressed(Some(Hit::CloseButton(0)), &frame, Some(0))
                .redraw
        );
        assert_eq!(pointer.close_state(), CloseButtonVisualState::Pressed);
        // A held close button keeps hovering from selecting rows.
        assert_eq!(
            pointer
                .moved(Some(Hit::Row(1)), AWAY, &frame, Some(0))
                .select,
            None
        );
        assert_eq!(
            pointer.released(Some(Hit::CloseButton(0))),
            Response {
                action: Some(Action::Command(WindowCommand::Close)),
                redraw: true,
                ..Response::default()
            }
        );
        assert_eq!(pointer.close_state(), CloseButtonVisualState::Normal);

        let _ = pointer.pressed(Some(Hit::CloseButton(0)), &frame, Some(0));
        assert_eq!(
            pointer.released(Some(Hit::Row(0))),
            Response {
                redraw: true,
                ..Response::default()
            }
        );
    }

    #[test]
    fn a_right_click_selects_what_it_is_on_for_the_menu() {
        let pointer = Pointer::default();
        let frame = shown(2, 1, Some(0));

        for hit in [Hit::Row(1), Hit::CloseButton(1)] {
            assert_eq!(
                pointer.right_pressed(Some(hit), &frame, Some(4)),
                Response {
                    select: Some((4, Some(2))),
                    menu: Some(MenuFor::Window),
                    ..Response::default()
                }
            );
        }
        assert_eq!(
            pointer.right_pressed(Some(Hit::Tile(1)), &frame, Some(4)),
            Response {
                dwell: Dwell::Cancel,
                select: Some((3, None)),
                menu: Some(MenuFor::App),
                ..Response::default()
            }
        );
        assert_eq!(
            pointer.right_pressed(None, &frame, Some(4)),
            Response::default()
        );
    }

    #[test]
    fn leaving_the_panel_lets_go_of_the_close_button_highlight() {
        let mut pointer = Pointer::default();
        let _ = pointer.moved(
            Some(Hit::CloseButton(0)),
            AWAY,
            &shown(0, 0, Some(0)),
            Some(0),
        );

        assert!(pointer.exited().redraw);
        assert_eq!(pointer.close_state(), CloseButtonVisualState::Normal);
        assert!(!pointer.exited().redraw);
    }
}
