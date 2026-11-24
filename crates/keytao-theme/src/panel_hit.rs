//! Candidate panel geometry shared by drawing and pointer hit testing.
//!
//! Pure math with no platform dependencies, so it is tested on every OS.
//! All values are buffer pixels: the renderer has already multiplied the theme
//! metrics by the HiDPI scale, and every backend attaches the buffer at scale 1,
//! so surface/window-local pointer coordinates are in the same space.

/// A rectangle in panel-local buffer pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl PanelRect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        let (x, y) = (x as f32, y as f32);
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// Where the candidates and paging buttons are drawn.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelHitAreas {
    /// Index-aligned with the candidates of the current page.
    pub candidates: Vec<PanelRect>,
    pub previous_page: Option<PanelRect>,
    pub next_page: Option<PanelRect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelClick {
    Candidate(usize),
    PreviousPage,
    NextPage,
}

impl PanelHitAreas {
    pub fn click_at(&self, x: f64, y: f64) -> Option<PanelClick> {
        if self.previous_page.is_some_and(|r| r.contains(x, y)) {
            return Some(PanelClick::PreviousPage);
        }
        if self.next_page.is_some_and(|r| r.contains(x, y)) {
            return Some(PanelClick::NextPage);
        }
        self.candidates
            .iter()
            .position(|r| r.contains(x, y))
            .map(PanelClick::Candidate)
    }
}

/// Inputs of the candidate layout, already scaled to buffer pixels.
pub struct PanelGeometry<'a> {
    pub vertical: bool,
    pub pad_x: f32,
    /// Top of the first candidate (padding plus the preedit line).
    pub top: f32,
    pub gap: f32,
    pub option_widths: &'a [f32],
    /// Optional per-option heights for vertical panels.
    pub option_heights: Option<&'a [f32]>,
    pub option_height: f32,
    pub nav_button: f32,
    pub can_go_previous: bool,
    pub can_go_next: bool,
}

/// Allocate measured row widths, including each item's own padding. The available
/// width excludes the panel chrome, gaps and page buttons. Keep indices intact.
pub fn fit_candidate_widths(
    natural: &[f32],
    minimum: &[f32],
    selected: usize,
    available: f32,
) -> Vec<f32> {
    assert_eq!(natural.len(), minimum.len());
    let available = available.max(0.0);
    if natural.iter().sum::<f32>() <= available {
        return natural.to_vec();
    }
    let selected = selected.min(natural.len() - 1);
    let mut widths: Vec<_> = natural.iter().zip(minimum).map(|(natural, minimum)| {
        minimum.min(*natural)
    }).collect();
    let mut distant: Vec<_> = (0..widths.len())
        .filter(|index| index.abs_diff(selected) > 1)
        .collect();
    distant.sort_by_key(|index| index.abs_diff(selected));
    let mut used = widths.iter().sum::<f32>();
    for index in distant.iter().rev().copied() {
        if used <= available {
            break;
        }
        used -= widths[index];
        widths[index] = 0.0;
    }
    if used > available {
        for width in &mut widths {
            *width *= available / used;
        }
        return widths;
    }

    // Share the remaining text room among the highlighted item and its two
    // neighbors, reclaiming the share of any short item that already fits.
    let mut nearby: Vec<_> = (selected.saturating_sub(1)..=(selected + 1).min(natural.len() - 1))
        .map(|index| (index, if index == selected { 2.0 } else { 1.0 }))
        .collect();
    nearby.sort_by(|&(left, left_weight), &(right, right_weight)| {
        ((natural[left] - widths[left]) / left_weight)
            .total_cmp(&((natural[right] - widths[right]) / right_weight))
    });
    let mut remaining = available - used;
    let mut weight = nearby.iter().map(|(_, weight)| weight).sum::<f32>();
    for (index, share) in nearby {
        let added = (remaining * share / weight).min(natural[index] - widths[index]);
        widths[index] += added;
        remaining = (remaining - added).max(0.0);
        weight -= share;
    }
    for index in distant {
        if widths[index] == 0.0 {
            continue;
        }
        let added = remaining.min(natural[index] - widths[index]);
        widths[index] += added;
        remaining -= added;
    }
    widths
}

/// The single source of candidate and paging-button positions: the renderer
/// draws into these rects and the backends hit-test against them.
pub fn layout(g: &PanelGeometry) -> PanelHitAreas {
    let mut areas = PanelHitAreas::default();
    let (mut x, mut y) = (g.pad_x, g.top);
    let vertical_heights = g.option_heights.filter(|_| g.vertical).unwrap_or_default();
    for (index, &width) in g.option_widths.iter().enumerate() {
        let height = vertical_heights
            .get(index)
            .copied()
            .unwrap_or(g.option_height);
        areas.candidates.push(PanelRect {
            x,
            y,
            width,
            height,
        });
        if g.vertical {
            y += height + g.gap;
        } else if width > 0.0 {
            x += width + g.gap;
        }
    }
    if !g.vertical {
        x += g.gap;
    }
    let button = |x| PanelRect {
        x,
        y,
        width: g.nav_button,
        height: g.nav_button,
    };
    if g.can_go_previous {
        areas.previous_page = Some(button(x));
        x += g.nav_button + g.gap;
    }
    if g.can_go_next {
        areas.next_page = Some(button(x));
    }
    areas
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflowing_candidates_allocate_room_around_the_highlight() {
        let natural = [900.0, 130.0, 900.0, 120.0, 800.0];
        let minimum = [26.0, 42.0, 42.0, 42.0, 26.0];
        assert_eq!(fit_candidate_widths(&natural, &minimum, 2, 4000.0), natural);
        let fitted = fit_candidate_widths(&natural, &minimum, 2, 260.0);
        assert!(fitted[2] > fitted[1] && fitted[1] > fitted[0]);
        assert!(fitted[3] > fitted[4]);
        assert_eq!((fitted[0], fitted[4]), (26.0, 26.0));
        assert!((fitted.iter().sum::<f32>() - 260.0).abs() < 0.01);

        let narrow = fit_candidate_widths(&natural, &minimum, 2, 126.0);
        assert_eq!(narrow, [0.0, 42.0, 42.0, 42.0, 0.0]);
        let hit = horizontal(&narrow, 1.0, true);
        assert_eq!(hit.click_at(11.0, 10.0), Some(PanelClick::Candidate(1)));
        assert_eq!(hit.click_at(57.0, 10.0), Some(PanelClick::Candidate(2)));
    }

    /// Mirrors how `PanelRenderer::render` scales theme metrics (`s()`).
    fn horizontal(widths: &[f32], scale: f32, next: bool) -> PanelHitAreas {
        let scaled: Vec<f32> = widths.iter().map(|w| w * scale).collect();
        layout(&PanelGeometry {
            vertical: false,
            pad_x: 10.0 * scale,
            top: 8.0 * scale,
            gap: 4.0 * scale,
            option_widths: &scaled,
            option_heights: None,
            option_height: 30.0 * scale,
            nav_button: 20.0 * scale,
            can_go_previous: false,
            can_go_next: next,
        })
    }

    #[test]
    fn horizontal_points_map_to_candidates_and_gaps_miss() {
        let areas = horizontal(&[50.0, 40.0], 1.0, false);
        // Candidate 0 spans x 10..60, gap 60..64, candidate 1 spans 64..104.
        assert_eq!(areas.click_at(10.0, 8.0), Some(PanelClick::Candidate(0)));
        assert_eq!(areas.click_at(59.9, 37.9), Some(PanelClick::Candidate(0)));
        assert_eq!(areas.click_at(62.0, 20.0), None);
        assert_eq!(areas.click_at(64.0, 20.0), Some(PanelClick::Candidate(1)));
        assert_eq!(areas.click_at(30.0, 4.0), None); // top padding
        assert_eq!(areas.click_at(30.0, 38.0), None); // below the row
        assert_eq!(areas.click_at(5.0, 20.0), None); // left padding
    }

    #[test]
    fn hidpi_scale_moves_rects_with_the_drawing() {
        let areas = horizontal(&[50.0, 40.0], 2.0, false);
        // At 2x, candidate 1 spans x 128..208 and the gap is 120..128.
        assert_eq!(areas.click_at(124.0, 40.0), None);
        assert_eq!(areas.click_at(130.0, 40.0), Some(PanelClick::Candidate(1)));
        assert_eq!(areas.click_at(110.0, 40.0), Some(PanelClick::Candidate(0)));
        assert_eq!(areas.click_at(130.0, 77.0), None); // below 16 + 60
    }

    #[test]
    fn vertical_rows_stack_with_gaps() {
        let areas = layout(&PanelGeometry {
            vertical: true,
            pad_x: 10.0,
            top: 8.0,
            gap: 4.0,
            option_widths: &[50.0, 80.0],
            option_heights: None,
            option_height: 30.0,
            nav_button: 20.0,
            can_go_previous: true,
            can_go_next: true,
        });
        assert_eq!(areas.click_at(20.0, 20.0), Some(PanelClick::Candidate(0)));
        assert_eq!(areas.click_at(20.0, 40.0), None); // gap 38..42
        assert_eq!(areas.click_at(80.0, 50.0), Some(PanelClick::Candidate(1)));
        assert_eq!(areas.click_at(70.0, 20.0), None); // past row 0's width
                                                      // Navigation row at y 76, buttons at x 10 and 34.
        assert_eq!(areas.click_at(15.0, 80.0), Some(PanelClick::PreviousPage));
        assert_eq!(areas.click_at(40.0, 80.0), Some(PanelClick::NextPage));
        assert_eq!(areas.click_at(32.0, 80.0), None);
    }

    #[test]
    fn vertical_rows_and_paging_follow_individual_heights() {
        let areas = layout(&PanelGeometry {
            vertical: true,
            pad_x: 10.0,
            top: 8.0,
            gap: 4.0,
            option_widths: &[50.0, 80.0],
            option_heights: Some(&[30.0, 50.0]),
            option_height: 50.0,
            nav_button: 20.0,
            can_go_previous: false,
            can_go_next: true,
        });
        assert_eq!(areas.click_at(20.0, 37.0), Some(PanelClick::Candidate(0)));
        assert_eq!(areas.click_at(20.0, 40.0), None);
        assert_eq!(areas.click_at(20.0, 91.0), Some(PanelClick::Candidate(1)));
        assert_eq!(areas.click_at(20.0, 96.0), Some(PanelClick::NextPage));
    }

    #[test]
    fn next_page_button_follows_the_last_candidate() {
        let areas = horizontal(&[50.0], 1.0, true);
        // Candidate 10..60, then gap + extra gap: button at 68..88.
        assert_eq!(areas.click_at(66.0, 10.0), None);
        assert_eq!(areas.click_at(70.0, 10.0), Some(PanelClick::NextPage));
        assert_eq!(areas.previous_page, None);
    }

    #[test]
    fn no_candidates_means_no_hits() {
        let areas = horizontal(&[], 1.0, false);
        assert_eq!(areas.click_at(10.0, 10.0), None);
        assert_eq!(PanelHitAreas::default().click_at(0.0, 0.0), None);
    }
}
