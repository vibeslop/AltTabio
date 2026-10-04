//! How the close button on the selected window row draws: at rest, under the pointer, or held
//! down.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CloseButtonVisualState {
    #[default]
    Normal,
    Hovered,
    Pressed,
}
