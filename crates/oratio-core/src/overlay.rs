//! Where the little wave overlay appears on screen: eight preset spots, or a spot the user
//! dragged it to. Pure geometry, so it can be tested without a display.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    TopLeft,
    TopCenter,
    TopRight,
    MiddleLeft,
    MiddleRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Preset {
    pub fn parse(id: &str) -> Option<Preset> {
        Some(match id {
            "top_left" => Preset::TopLeft,
            "top_center" => Preset::TopCenter,
            "top_right" => Preset::TopRight,
            "middle_left" => Preset::MiddleLeft,
            "middle_right" => Preset::MiddleRight,
            "bottom_left" => Preset::BottomLeft,
            "bottom_center" => Preset::BottomCenter,
            "bottom_right" => Preset::BottomRight,
            _ => return None,
        })
    }
}

/// Where the overlay goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Preset(Preset),
    /// The window's top-left corner, in screen pixels, where the user dragged it.
    Custom(i32, i32),
}

impl Default for Position {
    fn default() -> Self {
        Position::Preset(Preset::TopCenter)
    }
}

/// Top-left corner for a window of `size` (width, height) at a preset spot on `screen`.
pub fn place(preset: Preset, screen: Rect, size: (i32, i32), margin: i32) -> (i32, i32) {
    let (w, h) = size;
    let left = screen.x + margin;
    let right = screen.x + screen.w - w - margin;
    let center_x = screen.x + (screen.w - w) / 2;
    let top = screen.y + margin;
    let bottom = screen.y + screen.h - h - margin;
    let middle_y = screen.y + (screen.h - h) / 2;
    match preset {
        Preset::TopLeft => (left, top),
        Preset::TopCenter => (center_x, top),
        Preset::TopRight => (right, top),
        Preset::MiddleLeft => (left, middle_y),
        Preset::MiddleRight => (right, middle_y),
        Preset::BottomLeft => (left, bottom),
        Preset::BottomCenter => (center_x, bottom),
        Preset::BottomRight => (right, bottom),
    }
}

/// True if the window's centre lies on one of the screens (so a remembered spot on a monitor that
/// has since been unplugged is not used).
pub fn is_visible(pos: (i32, i32), size: (i32, i32), screens: &[Rect]) -> bool {
    let (cx, cy) = (pos.0 + size.0 / 2, pos.1 + size.1 / 2);
    screens.iter().any(|s| cx >= s.x && cx < s.x + s.w && cy >= s.y && cy < s.y + s.h)
}

/// Resolves a [`Position`] to a top-left corner, falling back to the top centre of `primary` when a
/// dragged spot is no longer on any screen.
pub fn resolve(position: Position, primary: Rect, screens: &[Rect], size: (i32, i32), margin: i32) -> (i32, i32) {
    match position {
        Position::Preset(p) => place(p, primary, size, margin),
        Position::Custom(x, y) if is_visible((x, y), size, screens) => (x, y),
        Position::Custom(..) => place(Preset::TopCenter, primary, size, margin),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect { x: 0, y: 0, w: 1920, h: 1080 };
    const SIZE: (i32, i32) = (400, 200);

    #[test]
    fn presets_land_on_the_right_edges_with_a_margin() {
        assert_eq!(place(Preset::TopLeft, SCREEN, SIZE, 24), (24, 24));
        assert_eq!(place(Preset::TopCenter, SCREEN, SIZE, 24), (760, 24));
        assert_eq!(place(Preset::TopRight, SCREEN, SIZE, 24), (1496, 24));
        assert_eq!(place(Preset::MiddleLeft, SCREEN, SIZE, 24), (24, 440));
        assert_eq!(place(Preset::BottomRight, SCREEN, SIZE, 24), (1496, 856));
        assert_eq!(place(Preset::BottomCenter, SCREEN, SIZE, 24), (760, 856));
    }

    #[test]
    fn a_second_monitor_offset_is_respected() {
        let right_monitor = Rect { x: 1920, y: -200, w: 2560, h: 1440 };
        assert_eq!(place(Preset::TopLeft, right_monitor, SIZE, 24), (1944, -176));
    }

    #[test]
    fn a_dragged_spot_is_used_while_it_is_on_screen() {
        assert_eq!(resolve(Position::Custom(100, 300), SCREEN, &[SCREEN], SIZE, 24), (100, 300));
    }

    #[test]
    fn a_dragged_spot_on_an_unplugged_monitor_falls_back_to_the_top_centre() {
        assert_eq!(resolve(Position::Custom(2500, 300), SCREEN, &[SCREEN], SIZE, 24), (760, 24));
        assert!(!is_visible((-2000, 0), SIZE, &[SCREEN]));
    }

    #[test]
    fn preset_ids_round_trip_and_unknown_ids_are_rejected() {
        assert_eq!(Preset::parse("bottom_left"), Some(Preset::BottomLeft));
        assert_eq!(Preset::parse("nowhere"), None);
        assert_eq!(Position::default(), Position::Preset(Preset::TopCenter));
    }
}
