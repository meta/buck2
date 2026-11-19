/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Shared viewport gestures for the critical-path strip and allocation shares.
//! Counter composition is drawn by detail_ui, not a second unused renderer.

/// The bar's own viewport, kept per open popup so zooming one action and
/// closing it does not zoom the next one.
#[derive(Clone, Debug, Default)]
pub struct BarState {
    /// left edge of the view, in bar units
    pub v0: f64,
    /// view width in bar units; `None` means "the whole bar"
    pub vw: Option<f64>,
}

impl BarState {
    /// The view as an explicit range, resolving the fitted case.
    pub(crate) fn view(&self, denom: i64) -> (f64, f64) {
        let full = denom.max(1) as f64;
        let w = self.vw.unwrap_or(full).clamp(full / 20_000.0, full);
        let v0 = self.v0.clamp(0.0, (full - w).max(0.0));
        (v0, w)
    }

    /// Zoom by `factor` keeping the bar unit under `frac` (0..1 across
    /// the rect) where it is.
    fn zoom_at(&mut self, factor: f64, frac: f64, denom: i64) {
        let full = denom.max(1) as f64;
        let (v0, w) = self.view(denom);
        let anchor = v0 + w * frac.clamp(0.0, 1.0);
        let nw = (w / factor).clamp(full / 20_000.0, full);
        self.v0 = (anchor - nw * frac.clamp(0.0, 1.0)).clamp(0.0, (full - nw).max(0.0));
        self.vw = Some(nw);
    }

    pub fn fit(&mut self) {
        self.v0 = 0.0;
        self.vw = None;
    }

    /// Bring `t0..t0+len` into view without changing the zoom — walking
    /// to something that scrolled off screen is walking to something
    /// nobody can see.
    pub(crate) fn reveal_span(&mut self, t0: i64, len: i64, denom: i64) {
        let full = denom.max(1) as f64;
        let (v0, w) = self.view(denom);
        let (a, b) = (t0 as f64, t0.saturating_add(len.max(0)) as f64);
        let nv0 = if a < v0 {
            a
        } else if b > v0 + w {
            b - w
        } else {
            return;
        };
        self.v0 = nv0.clamp(0.0, (full - w).max(0.0));
        self.vw = Some(w);
    }
}

/// How a bar viewport is driven: drag to pan, pinch or wheel to zoom
/// about the pointer, shift-wheel to pan, double-click to fit.
///
/// Any strip laid out along a [`BarState`] calls this, so the gestures
/// cannot drift apart between one and the next. `resp` must have been
/// allocated with `Sense::click_and_drag()`.
pub(crate) fn gestures(
    ui: &egui::Ui,
    resp: &egui::Response,
    rect: egui::Rect,
    denom: i64,
    st: &mut BarState,
) {
    let full = denom.max(1) as f64;
    if resp.double_clicked() {
        st.fit();
    }
    let (v0, vw) = st.view(denom);
    if resp.dragged() {
        st.v0 = (v0 - resp.drag_delta().x as f64 / rect.width().max(1.0) as f64 * vw)
            .clamp(0.0, (full - vw).max(0.0));
        st.vw = Some(vw);
    }
    if resp.hovered() {
        let (scroll, zoom, pos) =
            ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
        let frac = pos
            .map(|p| ((p.x - rect.left()) / rect.width().max(1.0)) as f64)
            .unwrap_or(0.5);
        if (zoom - 1.0).abs() > 1e-4 {
            st.zoom_at(zoom as f64, frac, denom);
        } else if scroll.y.abs() > 0.5 {
            // a wheel mouse has no pinch to send; over the bar the wheel
            // is the zoom, which is why this is gated on hover
            st.zoom_at((1.0 + scroll.y as f64 / 400.0).clamp(0.2, 5.0), frac, denom);
        }
        if scroll.x.abs() > 0.5 {
            let (v0, vw) = st.view(denom);
            st.v0 = (v0 - scroll.x as f64 / rect.width().max(1.0) as f64 * vw)
                .clamp(0.0, (full - vw).max(0.0));
            st.vw = Some(vw);
        }
    }
}

/// A segment that holds time but rounds below one percent reads `<1%`,
/// so it is not confused with one that holds none. Carries its own `%`.
pub(super) fn share(us: i64, dur: i64) -> String {
    let p = 100i128 * i128::from(us.max(0)) / i128::from(dur.max(1));
    if p == 0 && us > 0 {
        "<1%".to_owned()
    } else {
        format!("{p}%")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sliver_that_holds_time_is_not_drawn_as_zero_percent() {
        assert_eq!(share(0, 1_000), "0%");
        assert_eq!(share(1, 1_000), "<1%");
        assert_eq!(share(310, 1_000), "31%");
        assert_eq!(share(0, 0), "0%");
        assert_eq!(share(i64::MAX, i64::MAX), "100%");
        assert_eq!(share(-1, 1), "0%");
    }

    #[test]
    fn zoom_keeps_the_unit_under_the_cursor() {
        let mut st = BarState::default();
        let denom = 1_000_000;
        st.zoom_at(4.0, 0.5, denom);
        let (v0, vw) = st.view(denom);
        assert!((v0 + vw * 0.5 - 500_000.0).abs() < 1.0);
        assert!((vw - 250_000.0).abs() < 1.0);
        st.zoom_at(0.001, 0.0, denom);
        let (v0, vw) = st.view(denom);
        assert_eq!(v0, 0.0);
        assert!((vw - denom as f64).abs() < 1.0);
    }

    #[test]
    fn reveal_scrolls_a_selected_span_into_view() {
        let mut st = BarState::default();
        let denom = 1_000_000;
        st.zoom_at(10.0, 0.0, denom);
        st.reveal_span(900_000, 1_000, denom);
        let (v0, vw) = st.view(denom);
        assert!(v0 <= 900_000.0 && 901_000.0 <= v0 + vw + 1.0);
        st.reveal_span(i64::MAX - 1, i64::MAX, i64::MAX);
        let (v0, vw) = st.view(i64::MAX);
        assert!(v0.is_finite() && vw.is_finite());
    }
}
