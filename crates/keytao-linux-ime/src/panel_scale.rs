//! Pure scale selection and logical-to-physical pixel conversion.

pub fn select_scale(
    override_scale: Option<f32>,
    fractional_scale: Option<f32>,
    preferred_integer_scale: Option<i32>,
    entered_output_scales: impl IntoIterator<Item = i32>,
) -> f32 {
    let scale = override_scale
        .or(fractional_scale)
        .or(preferred_integer_scale.map(|scale| scale as f32))
        .or_else(|| {
            entered_output_scales
                .into_iter()
                .max()
                .map(|scale| scale as f32)
        })
        .unwrap_or(1.0);
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

pub fn logical_to_physical(logical_size: u32, scale: f32) -> u32 {
    let scale = select_scale(Some(scale), None, None, []);
    (logical_size as f64 * scale as f64).round() as u32
}

#[cfg(test)]
mod tests {
    use super::{logical_to_physical, select_scale};

    #[test]
    fn scale_selection_prefers_override_then_fractional_integer_and_outputs() {
        assert_eq!(select_scale(Some(1.25), Some(1.5), Some(2), [3, 4]), 1.25);
        assert_eq!(select_scale(None, Some(1.5), Some(2), [3, 4]), 1.5);
        assert_eq!(select_scale(None, None, Some(2), [3, 4]), 2.0);
        assert_eq!(select_scale(None, None, None, [3, 4]), 4.0);
        assert_eq!(select_scale(None, None, None, []), 1.0);
        for invalid in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(select_scale(Some(invalid), Some(1.5), Some(2), [3]), 1.0);
            assert_eq!(select_scale(None, Some(invalid), Some(2), [3]), 1.0);
        }
        assert_eq!(select_scale(None, None, Some(0), [3]), 1.0);
        assert_eq!(select_scale(None, None, None, [0]), 1.0);
    }

    #[test]
    fn logical_to_physical_rounds_fractional_pixels_half_away_from_zero() {
        assert_eq!(logical_to_physical(101, 1.25), 126);
        assert_eq!(logical_to_physical(102, 1.25), 128);
        assert_eq!(logical_to_physical(103, 1.25), 129);
        assert_eq!(logical_to_physical(100, 1.25), 125);
        assert_eq!(logical_to_physical(101, 2.0), 202);
        assert_eq!(logical_to_physical(0, 1.25), 0);
        assert_eq!(logical_to_physical(101, f32::NAN), 101);
        assert_eq!(logical_to_physical(101, 0.0), 101);
    }
}
