//! Helpers shared by the two Wayland input-method protocols.

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    os::fd::FromRawFd,
    time::{Duration, Instant},
};

use wayland_client::{
    globals::GlobalList,
    protocol::{
        wl_output::{self, WlOutput},
        wl_registry::WlRegistry,
        wl_surface::{self, WlSurface},
    },
    Dispatch, Proxy, QueueHandle, WEnum,
};
use wayland_protocols::wp::{
    fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        wp_fractional_scale_v1::{self, WpFractionalScaleV1},
    },
    viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::ZxdgOutputManagerV1, zxdg_output_v1::ZxdgOutputV1,
};
use xkbcommon::xkb;

use crate::panel_scale::select_scale;

struct PanelOutput {
    output: WlOutput,
    scale: i32,
    size: (i32, i32),
    rotated: bool,
    xdg: Option<ZxdgOutputV1>,
    logical_width: Option<i32>,
}

pub struct WaylandPanelScale {
    override_scale: Option<f32>,
    fractional_manager: Option<WpFractionalScaleManagerV1>,
    viewporter: Option<WpViewporter>,
    fractional: Option<WpFractionalScaleV1>,
    viewport: Option<WpViewport>,
    fractional_scale: Option<f32>,
    preferred_integer_scale: Option<i32>,
    output_manager: Option<ZxdgOutputManagerV1>,
    outputs: HashMap<u32, PanelOutput>,
    entered_outputs: HashSet<u32>,
}

impl Default for WaylandPanelScale {
    fn default() -> Self {
        Self {
            // An unparsable override must not silently disable output-scale following.
            override_scale: std::env::var("KEYTAO_IME_PANEL_SCALE")
                .ok()
                .and_then(|value| value.trim().parse::<f32>().ok())
                .filter(|scale| scale.is_finite() && *scale > 0.0)
                .map(|scale| scale.clamp(0.75, 4.0)),
            fractional_manager: None,
            viewporter: None,
            fractional: None,
            viewport: None,
            fractional_scale: None,
            preferred_integer_scale: None,
            output_manager: None,
            outputs: HashMap::new(),
            entered_outputs: HashSet::new(),
        }
    }
}

impl WaylandPanelScale {
    pub fn bind_initial<D>(&mut self, globals: &GlobalList, qh: &QueueHandle<D>)
    where
        D: Dispatch<WlOutput, u32>
            + Dispatch<ZxdgOutputManagerV1, ()>
            + Dispatch<ZxdgOutputV1, u32>
            + Dispatch<WpFractionalScaleManagerV1, ()>
            + Dispatch<WpViewporter, ()>
            + 'static,
    {
        // registry_queue_init does not forward the initial globals to Dispatch.
        for global in globals.contents().clone_list() {
            self.bind_global(
                globals.registry(),
                global.name,
                &global.interface,
                global.version,
                qh,
            );
        }
    }

    pub fn bind_global<D>(
        &mut self,
        registry: &WlRegistry,
        name: u32,
        interface: &str,
        version: u32,
        qh: &QueueHandle<D>,
    ) -> bool
    where
        D: Dispatch<WlOutput, u32>
            + Dispatch<ZxdgOutputManagerV1, ()>
            + Dispatch<ZxdgOutputV1, u32>
            + Dispatch<WpFractionalScaleManagerV1, ()>
            + Dispatch<WpViewporter, ()>
            + 'static,
    {
        if version == 0 {
            return false;
        }
        match interface {
            "wl_output" => {
                self.outputs.entry(name).or_insert_with(|| PanelOutput {
                    output: registry.bind(name, version.min(4), qh, name),
                    scale: 1,
                    size: (0, 0),
                    rotated: false,
                    xdg: None,
                    logical_width: None,
                });
            }
            "zxdg_output_manager_v1" => {
                self.output_manager = Some(registry.bind(name, version.min(3), qh, ()));
            }
            "wp_fractional_scale_manager_v1" => {
                self.fractional_manager = Some(registry.bind(name, version.min(1), qh, ()));
            }
            "wp_viewporter" => {
                self.viewporter = Some(registry.bind(name, version.min(1), qh, ()));
            }
            _ => return false,
        }
        if let Some(manager) = &self.output_manager {
            for (&name, output) in &mut self.outputs {
                if output.xdg.is_none() {
                    output.xdg = Some(manager.get_xdg_output(&output.output, qh, name));
                }
            }
        }
        true
    }

    pub fn remove_global(&mut self, name: u32) {
        if let Some(output) = self.outputs.remove(&name) {
            self.entered_outputs.remove(&name);
            if let Some(xdg) = output.xdg {
                xdg.destroy();
            }
            if output.output.version() >= 3 {
                output.output.release();
            }
        }
    }

    pub fn attach_surface<D>(&mut self, surface: &WlSurface, qh: &QueueHandle<D>)
    where
        D: Dispatch<WpFractionalScaleV1, ()> + Dispatch<WpViewport, ()> + 'static,
    {
        if self.viewport.is_none() {
            self.viewport = self
                .viewporter
                .as_ref()
                .map(|viewporter| viewporter.get_viewport(surface, qh, ()));
        }
        if self.fractional.is_none() && self.viewport.is_some() {
            self.fractional = self
                .fractional_manager
                .as_ref()
                .map(|manager| manager.get_fractional_scale(surface, qh, ()));
        }
    }

    pub fn surface_event(&mut self, event: &wl_surface::Event) {
        match event {
            wl_surface::Event::Enter { output } => {
                if let Some(name) = output.data::<u32>() {
                    self.entered_outputs.insert(*name);
                }
            }
            wl_surface::Event::Leave { output } => {
                if let Some(name) = output.data::<u32>() {
                    self.entered_outputs.remove(name);
                }
            }
            wl_surface::Event::PreferredBufferScale { factor } => {
                self.preferred_integer_scale = Some(*factor);
            }
            _ => {}
        }
    }

    pub fn output_event(&mut self, output: &WlOutput, event: &wl_output::Event) {
        let Some(output) = output
            .data::<u32>()
            .and_then(|name| self.outputs.get_mut(name))
        else {
            return;
        };
        match event {
            wl_output::Event::Scale { factor } => output.scale = *factor,
            wl_output::Event::Mode {
                flags: WEnum::Value(flags),
                width,
                height,
                ..
            } if flags.contains(wl_output::Mode::Current) => output.size = (*width, *height),
            wl_output::Event::Geometry {
                transform: WEnum::Value(transform),
                ..
            } => {
                output.rotated = matches!(
                    transform,
                    wl_output::Transform::_90
                        | wl_output::Transform::_270
                        | wl_output::Transform::Flipped90
                        | wl_output::Transform::Flipped270
                );
            }
            _ => {}
        }
    }

    pub fn output_logical_width(&mut self, name: u32, width: i32) {
        if let Some(output) = self.outputs.get_mut(&name).filter(|_| width > 0) {
            output.logical_width = Some(width);
        }
    }

    pub fn screen_width(&self) -> f32 {
        self.outputs
            .iter()
            .filter(|(name, _)| {
                self.entered_outputs.is_empty() || self.entered_outputs.contains(name)
            })
            .filter_map(|(_, output)| {
                let width = output
                    .logical_width
                    .map(|width| width as f32)
                    .unwrap_or_else(|| {
                        let width = if output.rotated {
                            output.size.1
                        } else {
                            output.size.0
                        };
                        width as f32 / output.scale.max(1) as f32
                    });
                (width > 0.0).then_some(width)
            })
            .reduce(f32::min)
            .unwrap_or(1.0)
    }

    pub fn fractional_event(&mut self, event: &wp_fractional_scale_v1::Event) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            self.fractional_scale = Some(*scale as f32 / 120.0);
        }
    }

    pub fn scale(&self, surface: &WlSurface) -> f32 {
        let scale = select_scale(
            self.override_scale,
            self.fractional_scale,
            self.preferred_integer_scale,
            self.entered_outputs
                .iter()
                .filter_map(|name| self.outputs.get(name).map(|output| output.scale)),
        );
        if self.viewport_active() {
            scale
        } else if surface.version() >= 3 {
            scale.ceil()
        } else {
            1.0
        }
    }

    pub fn configure_surface(&self, surface: &WlSurface, logical_width: u32, logical_height: u32) {
        if self.viewport_active() {
            if surface.version() >= 3 {
                surface.set_buffer_scale(1);
            }
            if let Some(viewport) = &self.viewport {
                viewport.set_destination(logical_width as i32, logical_height as i32);
            }
        } else {
            if let Some(viewport) = &self.viewport {
                viewport.set_destination(-1, -1);
            }
            if surface.version() >= 3 {
                surface.set_buffer_scale(self.scale(surface) as i32);
            }
        }
    }

    fn viewport_active(&self) -> bool {
        self.viewport.is_some() && (self.fractional.is_some() || self.override_scale.is_some())
    }
}

/// Wayland preedit indices are byte offsets; `ImeState::cursor` counts scalars.
pub fn preedit_cursor_bytes(preedit: &str, cursor: usize) -> i32 {
    preedit
        .char_indices()
        .nth(cursor)
        .map(|(offset, _)| offset)
        .unwrap_or(preedit.len()) as i32
}

/// A zero rate disables repeating; negative protocol values are invalid.
pub fn repeat_timings(rate: i32, delay: i32) -> Option<(Duration, Duration)> {
    if rate <= 0 || delay < 0 {
        return None;
    }
    Some((
        Duration::from_millis(delay as u64),
        Duration::from_micros(1_000_000 / rate as u64),
    ))
}

struct RepeatingKey {
    key: u32,
    next: Instant,
    interval: Duration,
}

/// Keys consumed by the IME must repeat here rather than in the client.
#[derive(Default)]
pub struct KeyRepeat {
    timings: Option<(Duration, Duration)>,
    current: Option<RepeatingKey>,
}

impl KeyRepeat {
    pub fn set_info(&mut self, rate: i32, delay: i32) {
        self.timings = repeat_timings(rate, delay);
        if self.timings.is_none() {
            self.cancel();
        }
    }

    pub fn arm(&mut self, key: u32, repeats: bool) {
        self.cancel();
        if let Some((delay, interval)) = self.timings.filter(|_| repeats) {
            self.current = Some(RepeatingKey {
                key,
                next: Instant::now() + delay,
                interval,
            });
        }
    }

    pub fn cancel(&mut self) {
        self.current = None;
    }

    pub fn cancel_for(&mut self, key: u32) {
        if self.key() == Some(key) {
            self.cancel();
        }
    }

    pub fn key(&self) -> Option<u32> {
        self.current.as_ref().map(|repeat| repeat.key)
    }

    pub fn due(&self) -> bool {
        self.next_deadline()
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.current.as_ref().map(|repeat| repeat.next)
    }

    pub fn advance(&mut self, key: u32) {
        // The key handler re-arms the initial delay; subsequent presses use
        // the interval unless that handler cancelled or replaced the key.
        if let Some(repeat) = self.current.as_mut().filter(|repeat| repeat.key == key) {
            repeat.next = Instant::now() + repeat.interval;
        }
    }
}

pub fn key_sym(state: Option<&xkb::State>, evdev_keycode: u32) -> u32 {
    state
        .map(|state| state.key_get_one_sym(xkb::Keycode::from(evdev_keycode + 8)))
        .unwrap_or(xkb::Keysym::from(xkb::keysyms::KEY_NoSymbol))
        .into()
}

/// Modifiers and keys marked `repeat=no` must not repeat.
pub fn key_repeats(keymap: Option<&xkb::Keymap>, evdev_keycode: u32) -> bool {
    keymap.is_some_and(|keymap| keymap.key_repeats(xkb::Keycode::from(evdev_keycode + 8)))
}

pub fn mode_hint_deadline(duration: Option<Duration>) -> Instant {
    Instant::now() + duration.unwrap_or_else(|| Duration::from_millis(750))
}

pub fn deadline_active(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|deadline| Instant::now() < deadline)
}

pub fn take_expired_deadline(deadline: &mut Option<Instant>) -> bool {
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        *deadline = None;
        true
    } else {
        false
    }
}

/// Milliseconds until the next timer fires, or -1 when nothing is pending.
pub fn poll_timeout_ms(deadlines: impl IntoIterator<Item = Option<Instant>>) -> i32 {
    let Some(next) = deadlines.into_iter().flatten().min() else {
        return -1;
    };
    let now = Instant::now();
    if next <= now {
        return 0;
    }
    (next - now).as_millis().min(i32::MAX as u128) as i32
}

pub fn tempfile(flags: libc::c_uint) -> std::io::Result<File> {
    let fd = unsafe { libc::memfd_create(c"keytao-shm".as_ptr(), flags) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(test)]
mod tests {
    use super::{preedit_cursor_bytes, repeat_timings};
    use std::time::Duration;

    #[test]
    fn repeat_info_maps_to_a_delay_and_an_interval() {
        assert_eq!(
            repeat_timings(25, 600),
            Some((Duration::from_millis(600), Duration::from_millis(40)))
        );
        // A rate of zero disables repeating, as does an illegal negative one.
        assert_eq!(repeat_timings(0, 600), None);
        assert_eq!(repeat_timings(-1, 600), None);
        assert_eq!(repeat_timings(25, -1), None);
    }

    #[test]
    fn preedit_cursor_is_a_byte_offset() {
        assert_eq!(preedit_cursor_bytes("nihao", 2), 2);
        // Unicode scalar 2 of "你好ni" starts at byte 6.
        assert_eq!(preedit_cursor_bytes("你好ni", 2), 6);
        // Past the end clamps to the byte length rather than panicking.
        assert_eq!(preedit_cursor_bytes("你好", 9), 6);
        assert_eq!(preedit_cursor_bytes("", 0), 0);
    }
}
