//! Helpers shared by the two Wayland input-method protocols.

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    ops::Range,
    os::fd::{AsFd, FromRawFd},
    time::{Duration, Instant},
};

use memmap2::{MmapMut, MmapOptions};
use wayland_client::{
    globals::GlobalList,
    protocol::{
        wl_buffer::WlBuffer,
        wl_output::{self, WlOutput},
        wl_registry::WlRegistry,
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
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

#[derive(Default)]
pub struct WaylandPanelBuffers {
    pool: Option<(File, WlShmPool, MmapMut)>,
    slots: [Option<PanelBuffer>; 2],
}

struct PanelBuffer {
    proxy: WlBuffer,
    range: Range<usize>,
    width: u32,
    busy: bool,
}

impl WaylandPanelBuffers {
    pub fn available_slot(&self) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| slot.as_ref().is_none_or(|slot| !slot.busy))
    }

    pub fn release(&mut self, buffer: &WlBuffer) {
        if let Some(slot) = self
            .slots
            .iter_mut()
            .flatten()
            .find(|slot| slot.proxy == *buffer)
        {
            slot.busy = false;
        }
    }

    pub fn write<D>(
        &mut self,
        slot: usize,
        shm: &WlShm,
        qh: &QueueHandle<D>,
        size: (u32, u32),
        raster_width: u32,
        pixels: &[u8],
    ) -> std::io::Result<&WlBuffer>
    where
        D: Dispatch<WlShmPool, ()> + Dispatch<WlBuffer, ()> + 'static,
    {
        let (width, height) = size;
        let invalid_size = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid panel buffer size",
            )
        };
        let stride = width.checked_mul(4).ok_or_else(invalid_size)? as usize;
        let raster_stride = raster_width.checked_mul(4).ok_or_else(invalid_size)? as usize;
        let len = stride
            .checked_mul(height as usize)
            .ok_or_else(invalid_size)?;
        if raster_stride == 0
            || raster_stride > stride
            || pixels.len() % raster_stride != 0
            || pixels.len() / raster_stride > height as usize
            || len == 0
        {
            return Err(invalid_size());
        }

        let current = self.slots[slot].as_ref();
        assert!(current.is_none_or(|buffer| !buffer.busy));
        let other = self.slots[1 - slot].as_ref();
        let offset = panel_buffer_offset(
            current.map(|buffer| &buffer.range),
            other.map(|buffer| &buffer.range),
            len,
        );
        let end = offset
            .checked_add(len)
            .filter(|end| *end <= i32::MAX as usize)
            .ok_or_else(invalid_size)?;
        let capacity = end.next_power_of_two().min(i32::MAX as usize);
        match &mut self.pool {
            Some((file, pool, mapping)) if end > mapping.len() => {
                file.set_len(capacity as u64)?;
                // The memfd is owned here, grows monotonically, and the compositor only reads it.
                *mapping = unsafe { MmapOptions::new().len(capacity).map_mut(&*file)? };
                pool.resize(capacity as i32);
            }
            None => {
                let file = tempfile(libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING)?;
                file.set_len(capacity as u64)?;
                let mapping = unsafe { MmapOptions::new().len(capacity).map_mut(&file)? };
                let pool = shm.create_pool(file.as_fd(), capacity as i32, qh, ());
                self.pool = Some((file, pool, mapping));
            }
            _ => {}
        }
        let (_, pool, mapping) = self.pool.as_mut().unwrap();
        copy_panel_pixels(&mut mapping[offset..end], stride, pixels, raster_stride);

        let range = offset..end;
        let entry = &mut self.slots[slot];
        if !entry
            .as_ref()
            .is_some_and(|buffer| buffer.range == range && buffer.width == width)
        {
            if let Some(old) = entry.take() {
                old.proxy.destroy();
            }
            *entry = Some(PanelBuffer {
                proxy: pool.create_buffer(
                    offset as i32,
                    width as i32,
                    height as i32,
                    stride as i32,
                    wl_shm::Format::Argb8888,
                    qh,
                    (),
                ),
                range,
                width,
                busy: false,
            });
        }
        let buffer = entry.as_mut().unwrap();
        buffer.busy = true;
        Ok(&buffer.proxy)
    }
}

fn panel_buffer_offset(
    current: Option<&Range<usize>>,
    other: Option<&Range<usize>>,
    len: usize,
) -> usize {
    if let Some(range) = current.filter(|range| range.len() >= len) {
        return range.start;
    }
    other
        .filter(|range| range.start < len)
        .map_or(0, |range| range.end)
}

fn copy_panel_pixels(output: &mut [u8], stride: usize, pixels: &[u8], raster_stride: usize) {
    if stride == raster_stride {
        let (body, padding) = output.split_at_mut(pixels.len());
        body.copy_from_slice(pixels);
        padding.fill(0);
        return;
    }
    let mut rows = output.chunks_exact_mut(stride);
    for (source, row) in pixels.chunks_exact(raster_stride).zip(&mut rows) {
        let (body, padding) = row.split_at_mut(raster_stride);
        body.copy_from_slice(source);
        padding.fill(0);
    }
    for row in rows {
        row.fill(0);
    }
}

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
    use super::{copy_panel_pixels, panel_buffer_offset, preedit_cursor_bytes, repeat_timings};
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

    #[test]
    fn growing_panel_buffer_keeps_the_other_slot_intact() {
        assert_eq!(panel_buffer_offset(Some(&(0..4)), Some(&(4..12)), 8), 12);
        assert_eq!(panel_buffer_offset(Some(&(4..12)), Some(&(12..20)), 12), 0);
        assert_eq!(panel_buffer_offset(Some(&(12..20)), Some(&(0..12)), 8), 12);
    }

    #[test]
    fn copying_panel_pixels_clears_padding_from_a_reused_slot() {
        let mut output = [255; 18];
        copy_panel_pixels(&mut output, 6, &[1, 2, 3, 4, 5, 6, 7, 8], 4);
        assert_eq!(
            output,
            [1, 2, 3, 4, 0, 0, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0]
        );
    }
}
