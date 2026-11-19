//! Per-output camera. Pure maths, no Wayland types.
//! Screen position (logical px) = (world - (x, y)) * zoom.

use serde::{Deserialize, Serialize};

pub const MIN_ZOOM: f32 = 0.1;
pub const MAX_ZOOM: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    /// World coordinate at the top-left corner of the surface.
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}

impl Camera {
    /// Camera with world origin at the centre of a `w` x `h` (logical) surface.
    pub fn centered(w: f32, h: f32) -> Self {
        Camera {
            x: -w / 2.0,
            y: -h / 2.0,
            zoom: 1.0,
        }
    }

    /// True if every field is finite and the zoom is in range (guards against a bad save file).
    pub fn is_valid(&self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.zoom.is_finite()
            && (MIN_ZOOM..=MAX_ZOOM).contains(&self.zoom)
    }

    #[cfg(test)]
    pub fn world_to_screen(&self, wx: f32, wy: f32) -> (f32, f32) {
        ((wx - self.x) * self.zoom, (wy - self.y) * self.zoom)
    }

    pub fn screen_to_world(&self, sx: f32, sy: f32) -> (f32, f32) {
        (self.x + sx / self.zoom, self.y + sy / self.zoom)
    }

    /// Pan by a screen-space delta (content follows the pointer).
    pub fn pan_by_screen(&mut self, dx: f32, dy: f32) {
        self.x -= dx / self.zoom;
        self.y -= dy / self.zoom;
    }

    /// Multiply zoom by `factor`, keeping the world point under (sx, sy) fixed.
    pub fn zoom_at(&mut self, sx: f32, sy: f32, factor: f32) {
        let (wx, wy) = self.screen_to_world(sx, sy);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.x = wx - sx / self.zoom;
        self.y = wy - sy / self.zoom;
    }

    /// Frame a world rectangle inside a `vw` x `vh` (logical) view with a margin.
    pub fn fit(
        &mut self,
        (rx, ry, rw, rh): (f32, f32, f32, f32),
        vw: f32,
        vh: f32,
        margin: f32,
        max_zoom: f32,
    ) {
        let zx = (vw - 2.0 * margin) / rw.max(1.0);
        let zy = (vh - 2.0 * margin) / rh.max(1.0);
        self.zoom = zx.min(zy).min(max_zoom).clamp(MIN_ZOOM, MAX_ZOOM);
        self.x = rx + rw / 2.0 - vw / 2.0 / self.zoom;
        self.y = ry + rh / 2.0 - vh / 2.0 / self.zoom;
    }

    /// Centre a world rectangle in the view, zooming so it fits with room around it but
    /// never beyond `max_zoom` (so a small card is not blown up).
    pub fn focus(
        &mut self,
        (rx, ry, rw, rh): (f32, f32, f32, f32),
        vw: f32,
        vh: f32,
        max_zoom: f32,
    ) {
        let fit = (vw * 0.8 / rw.max(1.0)).min(vh * 0.8 / rh.max(1.0));
        self.zoom = fit.min(max_zoom).clamp(MIN_ZOOM, MAX_ZOOM);
        self.x = rx + rw / 2.0 - vw / 2.0 / self.zoom;
        self.y = ry + rh / 2.0 - vh / 2.0 / self.zoom;
    }

    /// Set zoom to 1.0 keeping the point under (sx, sy) fixed.
    #[cfg(test)]
    pub fn reset_zoom_at(&mut self, sx: f32, sy: f32) {
        self.zoom_at(sx, sy, 1.0 / self.zoom);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn roundtrip() {
        let c = Camera {
            x: 10.0,
            y: -5.0,
            zoom: 2.5,
        };
        let (sx, sy) = c.world_to_screen(33.0, 7.0);
        let (wx, wy) = c.screen_to_world(sx, sy);
        assert!(close(wx, 33.0) && close(wy, 7.0));
    }

    #[test]
    fn centered_puts_origin_mid_screen() {
        let c = Camera::centered(1920.0, 1080.0);
        assert_eq!(c.world_to_screen(0.0, 0.0), (960.0, 540.0));
    }

    #[test]
    fn zoom_keeps_cursor_point_fixed() {
        let mut c = Camera {
            x: 100.0,
            y: 50.0,
            zoom: 1.0,
        };
        let before = c.screen_to_world(400.0, 300.0);
        c.zoom_at(400.0, 300.0, 1.7);
        let after = c.screen_to_world(400.0, 300.0);
        assert!(close(before.0, after.0) && close(before.1, after.1));
        assert!(close(c.zoom, 1.7));
    }

    #[test]
    fn zoom_is_clamped() {
        let mut c = Camera::centered(100.0, 100.0);
        c.zoom_at(0.0, 0.0, 1000.0);
        assert_eq!(c.zoom, MAX_ZOOM);
        c.zoom_at(0.0, 0.0, 1e-6);
        assert_eq!(c.zoom, MIN_ZOOM);
    }

    #[test]
    fn pan_moves_content_with_pointer() {
        let mut c = Camera {
            x: 0.0,
            y: 0.0,
            zoom: 2.0,
        };
        let p0 = c.world_to_screen(10.0, 10.0);
        c.pan_by_screen(30.0, -20.0);
        let p1 = c.world_to_screen(10.0, 10.0);
        assert!(close(p1.0 - p0.0, 30.0) && close(p1.1 - p0.1, -20.0));
    }

    #[test]
    fn reset_zoom() {
        let mut c = Camera {
            x: 5.0,
            y: 5.0,
            zoom: 3.0,
        };
        let before = c.screen_to_world(10.0, 20.0);
        c.reset_zoom_at(10.0, 20.0);
        assert!(close(c.zoom, 1.0));
        let after = c.screen_to_world(10.0, 20.0);
        assert!(close(before.0, after.0) && close(before.1, after.1));
    }

    #[test]
    fn fit_centres_and_contains() {
        let mut c = Camera {
            x: 0.0,
            y: 0.0,
            zoom: 1.0,
        };
        c.fit((100.0, 100.0, 400.0, 200.0), 800.0, 600.0, 50.0, 8.0);
        // width-limited: (800-100)/400 = 1.75
        assert!(close(c.zoom, 1.75));
        let (cx, cy) = c.screen_to_world(400.0, 300.0);
        assert!(close(cx, 300.0) && close(cy, 200.0));
        let (sx0, _) = c.world_to_screen(100.0, 100.0);
        assert!(sx0 >= 50.0 - 1e-3);
    }

    #[test]
    fn focus_centres_and_limits_zoom() {
        let mut c = Camera {
            x: 0.0,
            y: 0.0,
            zoom: 3.0,
        };
        c.focus((1000.0, 500.0, 320.0, 300.0), 1920.0, 1080.0, 1.2);
        assert!(close(c.zoom, 1.2), "small card is not blown up: {}", c.zoom);
        let (cx, cy) = c.screen_to_world(960.0, 540.0);
        assert!(close(cx, 1160.0) && close(cy, 650.0));
        // a huge rect zooms out to fit
        c.focus((0.0, 0.0, 10000.0, 500.0), 1920.0, 1080.0, 1.2);
        assert!(c.zoom < 0.2 && c.zoom >= crate::camera::MIN_ZOOM);
    }

    #[test]
    fn fit_never_zooms_in_past_the_cap() {
        let mut c = Camera {
            x: 0.0,
            y: 0.0,
            zoom: 1.0,
        };
        c.fit((0.0, 0.0, 320.0, 108.0), 1920.0, 1080.0, 60.0, 1.2);
        assert!(
            close(c.zoom, 1.2),
            "a tiny board is not blown up 5x: {}",
            c.zoom
        );
        let (cx, cy) = c.screen_to_world(960.0, 540.0);
        assert!(close(cx, 160.0) && close(cy, 54.0), "still centred");
    }
}
