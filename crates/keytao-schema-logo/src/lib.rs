use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Read, Write},
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    time::{Duration, Instant},
};

use image::{DynamicImage, ImageFormat, ImageReader, RgbaImage};
use resvg::{tiny_skia, usvg};

const MAX_FILE_BYTES: u64 = 1024 * 1024;
const RENDER_STACK_BYTES: usize = 32 * 1024 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Limits {
    per_candidate: Duration,
    total: Duration,
    #[cfg(test)]
    before_render: Option<std::sync::Arc<dyn Fn(&[u8]) + Send + Sync>>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            per_candidate: Duration::from_secs(10),
            total: Duration::from_secs(30),
            #[cfg(test)]
            before_render: None,
        }
    }
}

pub fn render_schema_logos(user_dir: &Path, schemas: &[(String, String)], size_px: u32) {
    if let Err(error) = render_schema_logos_inner(user_dir, schemas, size_px, Limits::default()) {
        eprintln!("Schema logo rendering failed: {error}");
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && !matches!(name, "." | "..") && !name.contains(['/', '\\', '\0'])
}

fn render_schema_logos_inner(
    user_dir: &Path,
    schemas: &[(String, String)],
    size_px: u32,
    limits: Limits,
) -> io::Result<()> {
    let started = Instant::now();
    let root = user_dir.canonicalize()?;
    let output = root.join(".keytao-logo");
    fs::create_dir_all(&output)?;
    let output = output.canonicalize()?;
    if !output.starts_with(&root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Logo directory escapes user directory",
        ));
    }
    let marker = output.join(".rendering");
    let skipped_path = output.join(".skipped");
    match fs::read_to_string(&marker) {
        Ok(previous) => {
            let mut skipped = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&skipped_path)?;
            skipped.write_all(previous.as_bytes())?;
            if !previous.ends_with('\n') {
                skipped.write_all(b"\n")?;
            }
            skipped.sync_all()?;
            fs::remove_file(&marker)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut skipped: HashSet<String> = match fs::read_to_string(&skipped_path) {
        Ok(contents) => contents.lines().map(str::to_owned).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => HashSet::new(),
        Err(error) => return Err(error),
    };
    for entry in fs::read_dir(&output)? {
        if started.elapsed() >= limits.total {
            return Ok(());
        }
        let entry = entry?;
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "png")
        {
            if let Err(error) = fs::remove_file(entry.path()) {
                eprintln!(
                    "Could not remove old schema logo {:?}: {error}",
                    entry.path()
                );
            }
        }
    }
    for (schema_id, schema_name) in schemas {
        if started.elapsed() >= limits.total {
            break;
        }
        if !valid_name(schema_id) {
            continue;
        }
        if let Some(png) = render_schema_logo_inner(
            &root,
            schema_id,
            schema_name,
            size_px,
            &output,
            &mut skipped,
            &limits,
            started,
        )? {
            if let Err(error) = write_png(&output, schema_id, &png) {
                eprintln!("Could not write schema logo {schema_id:?}: {error}");
            }
        }
    }
    Ok(())
}

fn write_png(output: &Path, schema_id: &str, png: &[u8]) -> io::Result<()> {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = output.join(format!(".logo-{}-{sequence}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(png)?;
        file.sync_all()?;
        fs::rename(&temporary, output.join(format!("{schema_id}.png")))
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn render_schema_logo_inner(
    user_dir: &Path,
    schema_id: &str,
    schema_name: &str,
    size_px: u32,
    output: &Path,
    skipped: &mut HashSet<String>,
    limits: &Limits,
    started: Instant,
) -> io::Result<Option<Vec<u8>>> {
    let size = size_px.clamp(16, 256);
    let marker = output.join(".rendering");
    for name in [schema_name, schema_id, "logo"] {
        if !valid_name(name) {
            continue;
        }
        for extension in ["svg", "ico"] {
            if started.elapsed() >= limits.total {
                return Ok(None);
            }
            let Some(candidate) = read_candidate(user_dir, &format!("{name}.{extension}")) else {
                continue;
            };
            if skipped.contains(&candidate.fingerprint) {
                continue;
            }
            let mut file = File::create(&marker)?;
            writeln!(file, "{}", candidate.fingerprint)?;
            file.sync_all()?;
            if started.elapsed() >= limits.total {
                fs::remove_file(&marker)?;
                return Ok(None);
            }
            let (sender, receiver) = mpsc::channel();
            let bytes = candidate.bytes;
            #[cfg(test)]
            let before_render = limits.before_render.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("schema-logo-render".into())
                .stack_size(RENDER_STACK_BYTES)
                .spawn(move || {
                    let rendered = catch_unwind(AssertUnwindSafe(|| {
                        #[cfg(test)]
                        if let Some(before_render) = &before_render {
                            before_render(&bytes);
                        }
                        match extension {
                            "svg" => render_svg(&bytes, size),
                            _ => render_ico(&bytes, size),
                        }
                    }));
                    // Only the caller writes files, so a late result has no side effects.
                    let _ = sender.send(rendered);
                })
            {
                fs::remove_file(&marker)?;
                eprintln!("Could not start schema logo rendering: {error}");
                continue;
            }
            let timeout = limits
                .per_candidate
                .min(limits.total.saturating_sub(started.elapsed()));
            let rendered = receiver.recv_timeout(timeout);
            if matches!(rendered, Err(RecvTimeoutError::Timeout)) {
                let mut file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(output.join(".skipped"))?;
                writeln!(file, "{}", candidate.fingerprint)?;
                file.sync_all()?;
                skipped.insert(candidate.fingerprint);
            }
            fs::remove_file(&marker)?;
            match rendered {
                Ok(Ok(Some(png))) => return Ok(Some(png)),
                Ok(Ok(None)) => eprintln!("Unusable schema logo {:?}", candidate.path),
                Ok(Err(_)) => eprintln!("Schema logo renderer panicked for {:?}", candidate.path),
                Err(RecvTimeoutError::Timeout) => {
                    eprintln!("Schema logo rendering timed out for {:?}", candidate.path)
                }
                Err(RecvTimeoutError::Disconnected) => {
                    eprintln!(
                        "Schema logo rendering thread disconnected for {:?}",
                        candidate.path
                    )
                }
            }
        }
    }
    Ok(None)
}

struct Candidate {
    path: PathBuf,
    bytes: Vec<u8>,
    fingerprint: String,
}

fn read_candidate(root: &Path, filename: &str) -> Option<Candidate> {
    let path = root.join(filename).canonicalize().ok()?;
    if !path.starts_with(root) {
        return None;
    }
    let metadata = path.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    let file = File::open(&path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    let fingerprint = format!(
        "path={path:?}\tsize={}\tmtime={:?}",
        metadata.len(),
        metadata.modified().ok()?
    );
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= MAX_FILE_BYTES).then_some(Candidate {
        path,
        bytes,
        fingerprint,
    })
}

fn render_svg(bytes: &[u8], size: u32) -> Option<Vec<u8>> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return None;
    }
    let mut options = usvg::Options::default();
    options.image_href_resolver = usvg::ImageHrefResolver {
        resolve_data: Box::new(|_, _, _| None),
        resolve_string: Box::new(|_, _| None),
    };
    let tree = usvg::Tree::from_data(bytes, &options).ok()?;
    let source = tree.size();
    let scale = (size as f32 / source.width()).min(size as f32 / source.height());
    let transform = tiny_skia::Transform::from_row(
        scale,
        0.0,
        0.0,
        scale,
        (size as f32 - source.width() * scale) / 2.0,
        (size as f32 - source.height() * scale) / 2.0,
    );
    let mut pixmap = tiny_skia::Pixmap::new(size, size)?;
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    pixmap.encode_png().ok()
}

fn render_ico(bytes: &[u8], size: u32) -> Option<Vec<u8>> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Ico);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(1024);
    limits.max_image_height = Some(1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .ok()?
        .resize(size, size, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    let mut canvas = RgbaImage::new(size, size);
    image::imageops::replace(
        &mut canvas,
        &image,
        ((size - image.width()) / 2) as i64,
        ((size - image.height()) / 2) as i64,
    );
    let mut png = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(canvas)
        .write_to(&mut png, ImageFormat::Png)
        .ok()?;
    Some(png.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{codecs::ico::IcoEncoder, ExtendedColorType, ImageEncoder, Rgba};
    use std::fs;
    use tempfile::tempdir;

    fn render_schema_logo(
        user_dir: &Path,
        schema_id: &str,
        schema_name: &str,
        size_px: u32,
    ) -> Option<Vec<u8>> {
        render_schema_logos(user_dir, &[(schema_id.into(), schema_name.into())], size_px);
        if !valid_name(schema_id) {
            return None;
        }
        fs::read(
            user_dir
                .join(".keytao-logo")
                .join(format!("{schema_id}.png")),
        )
        .ok()
    }

    fn svg(color: &str) -> Vec<u8> {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16"><rect width="32" height="16" fill="{color}"/></svg>"#
        )
        .into_bytes()
    }

    fn ico(color: [u8; 4]) -> Vec<u8> {
        let image = RgbaImage::from_pixel(32, 16, Rgba(color));
        let mut bytes = Vec::new();
        IcoEncoder::new(&mut bytes)
            .write_image(image.as_raw(), 32, 16, ExtendedColorType::Rgba8)
            .unwrap();
        bytes
    }

    fn png(bytes: &[u8], size: u32) -> RgbaImage {
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        let decoded = image::load_from_memory_with_format(bytes, ImageFormat::Png)
            .unwrap()
            .to_rgba8();
        assert_eq!(decoded.dimensions(), (size, size));
        decoded
    }

    #[test]
    fn lookup_order() {
        let dir = tempdir().unwrap();
        let candidates = [
            ("Name.svg", svg("red")),
            ("Name.ico", ico([0, 255, 0, 255])),
            ("id.svg", svg("blue")),
            ("id.ico", ico([255, 255, 0, 255])),
            ("logo.svg", svg("magenta")),
            ("logo.ico", ico([0, 255, 255, 255])),
        ];
        for (name, bytes) in &candidates {
            fs::write(dir.path().join(name), bytes).unwrap();
        }
        for (name, bytes) in candidates {
            let expected = if name.ends_with(".svg") {
                render_svg(&bytes, 48)
            } else {
                render_ico(&bytes, 48)
            };
            assert_eq!(render_schema_logo(dir.path(), "id", "Name", 48), expected);
            fs::remove_file(dir.path().join(name)).unwrap();
        }
        assert!(render_schema_logo(dir.path(), "id", "Name", 48).is_none());
    }

    #[test]
    fn invalid_identifiers_are_skipped() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("user");
        fs::create_dir(&root).unwrap();
        fs::write(dir.path().join("escape.svg"), svg("red")).unwrap();
        for name in [
            "",
            ".",
            "..",
            "../escape",
            "sub/name",
            "sub\\name",
            "bad\0name",
        ] {
            assert!(render_schema_logo(&root, name, name, 48).is_none());
        }
        fs::write(root.join("id.svg"), svg("blue")).unwrap();
        assert!(render_schema_logo(&root, "id", "../escape", 48).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("user");
        fs::create_dir(&root).unwrap();
        let outside = dir.path().join("outside.svg");
        fs::write(&outside, svg("red")).unwrap();
        std::os::unix::fs::symlink(outside, root.join("Name.svg")).unwrap();
        assert!(render_schema_logo(&root, "id", "Name", 48).is_none());
        fs::write(root.join("logo.svg"), svg("blue")).unwrap();
        assert!(render_schema_logo(&root, "id", "Name", 48).is_some());
    }

    #[test]
    fn oversized_file_is_rejected() {
        let dir = tempdir().unwrap();
        let file = File::create(dir.path().join("Name.svg")).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
        assert!(render_schema_logo(dir.path(), "id", "Name", 48).is_none());
    }

    #[test]
    fn invalid_file_falls_through() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Name.svg"), b"invalid").unwrap();
        fs::write(dir.path().join("Name.ico"), b"invalid").unwrap();
        fs::write(dir.path().join("id.svg"), svg("blue")).unwrap();
        let bytes = render_schema_logo(dir.path(), "id", "Name", 48).unwrap();
        assert_eq!(png(&bytes, 48).get_pixel(24, 24).0, [0, 0, 255, 255]);
    }

    #[test]
    fn gzip_magic_falls_through() {
        let dir = tempdir().unwrap();
        // A valid gzip stream containing the red SVG, not just an invalid header.
        let gzip = b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x02\xff\x75\xcc\xdb\x0d\x80\x20\
            \x0c\x40\xd1\x55\x9a\x0e\x40\x11\x13\x3f\x0c\xb0\x8c\x20\x90\xe0\x23\xd0\
            \x58\xc7\x57\x07\xf0\xfb\x9e\x5c\xdb\xaf\x04\xf7\x56\xf7\xee\x30\x33\x9f\
            \x33\x91\x88\x28\x19\xd5\xd1\x12\x19\xad\x35\xbd\x02\x41\x4a\xe0\xec\x70\
            \x34\x08\x39\x96\x94\xd9\xe1\x30\xa1\xb7\x2d\x2e\xfc\x13\x61\x2d\xb5\x3a\
            \x6c\x31\x20\x79\xfb\x6d\xfc\x03\xfe\xa5\x8f\x3c\x6e\x00\x00\x00";
        fs::write(dir.path().join("Name.svg"), gzip).unwrap();
        fs::write(dir.path().join("id.svg"), svg("blue")).unwrap();
        let bytes = render_schema_logo(dir.path(), "id", "Name", 48).unwrap();
        assert_eq!(png(&bytes, 48).get_pixel(24, 24).0, [0, 0, 255, 255]);
    }

    #[test]
    fn pattern_reference_chain_renders_on_large_stack() {
        let dir = tempdir().unwrap();
        let mut source = String::from(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="32" height="16"><defs><pattern id="p0" width="32" height="16" patternUnits="userSpaceOnUse"><rect width="32" height="16" fill="red"/></pattern>"#,
        );
        for index in 1..=200 {
            source.push_str(&format!(
                r##"<pattern id="p{index}" xlink:href="#p{}"/>"##,
                index - 1
            ));
        }
        source.push_str(r#"</defs><rect width="32" height="16" fill="url(#p200)"/></svg>"#);
        fs::write(dir.path().join("Name.svg"), source).unwrap();
        let bytes = render_schema_logo(dir.path(), "id", "Name", 192).unwrap();
        assert_eq!(png(&bytes, 192).get_pixel(96, 96).0, [255, 0, 0, 255]);
    }

    #[test]
    fn doctype_internal_subset_renders() {
        let dir = tempdir().unwrap();
        let source = format!(
            "<!DOCTYPE svg [<!ELEMENT svg ANY>]>\n{}",
            String::from_utf8(svg("red")).unwrap()
        );
        fs::write(dir.path().join("Name.svg"), source).unwrap();
        let bytes = render_schema_logo(dir.path(), "id", "Name", 48).unwrap();
        assert_eq!(png(&bytes, 48).get_pixel(24, 24).0, [255, 0, 0, 255]);
    }

    #[test]
    fn leftover_rendering_skips_exact_source_and_changed_source_retries() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::write(root.join("Name.svg"), svg("red")).unwrap();
        fs::write(root.join("id.svg"), svg("blue")).unwrap();
        let output = root.join(".keytao-logo");
        fs::create_dir(&output).unwrap();
        let candidate = read_candidate(&root, "Name.svg").unwrap();
        fs::write(
            output.join(".rendering"),
            format!("{}\n", candidate.fingerprint),
        )
        .unwrap();
        let bytes = render_schema_logo(&root, "id", "Name", 48).unwrap();
        assert_eq!(png(&bytes, 48).get_pixel(24, 24).0, [0, 0, 255, 255]);
        assert!(!output.join(".rendering").exists());
        assert_eq!(
            fs::read_to_string(output.join(".skipped")).unwrap(),
            format!("{}\n", candidate.fingerprint)
        );
        let bytes = render_schema_logo(&root, "id", "Name", 48).unwrap();
        assert_eq!(png(&bytes, 48).get_pixel(24, 24).0, [0, 0, 255, 255]);
        fs::write(root.join("Name.svg"), svg("lime")).unwrap();
        let bytes = render_schema_logo(&root, "id", "Name", 48).unwrap();
        assert_eq!(png(&bytes, 48).get_pixel(24, 24).0, [0, 255, 0, 255]);
    }

    #[test]
    fn timed_out_candidate_is_skipped_and_falls_through_within_total_budget() {
        use std::sync::{Arc, Mutex};

        struct WorkerFinished(mpsc::Sender<()>);
        impl Drop for WorkerFinished {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }

        for total_expires in [false, true] {
            let dir = tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            let slow = svg("red");
            fs::write(root.join("Name.svg"), &slow).unwrap();
            fs::write(root.join("id.svg"), svg("blue")).unwrap();
            let candidate = read_candidate(&root, "Name.svg").unwrap();
            let (release, blocked) = mpsc::channel();
            let blocked = Mutex::new(blocked);
            let (finished, done) = mpsc::channel();
            let finished = WorkerFinished(finished);
            let limits = Limits {
                per_candidate: if total_expires {
                    Duration::from_secs(5)
                } else {
                    Duration::from_secs(1)
                },
                total: if total_expires {
                    Duration::from_millis(100)
                } else {
                    Duration::from_secs(5)
                },
                before_render: Some(Arc::new(move |bytes| {
                    let _ = &finished;
                    if bytes == slow {
                        blocked.lock().unwrap().recv().unwrap();
                    }
                })),
            };
            let started = Instant::now();
            let result = render_schema_logos_inner(
                &root,
                &[
                    ("id".into(), "Name".into()),
                    ("other".into(), "Name".into()),
                ],
                48,
                limits,
            );
            let elapsed = started.elapsed();
            // Unblock and await the detached worker even if an assertion below fails.
            release.send(()).unwrap();
            done.recv_timeout(Duration::from_secs(5)).unwrap();
            result.unwrap();
            assert!(elapsed < Duration::from_secs(3), "elapsed: {elapsed:?}");
            let output = root.join(".keytao-logo");
            assert!(!output.join(".rendering").exists());
            assert_eq!(
                fs::read_to_string(output.join(".skipped")).unwrap(),
                format!("{}\n", candidate.fingerprint)
            );
            if total_expires {
                assert!(!output.join("id.png").exists());
                assert!(!output.join("other.png").exists());
            } else {
                let bytes = fs::read(output.join("id.png")).unwrap();
                assert_eq!(png(&bytes, 48).get_pixel(24, 24).0, [0, 0, 255, 255]);
                assert!(!output.join("other.png").exists());
                let bytes = render_schema_logo(&root, "id", "Name", 48).unwrap();
                assert_eq!(png(&bytes, 48).get_pixel(24, 24).0, [0, 0, 255, 255]);
            }
        }
    }

    #[test]
    fn old_pngs_are_removed_when_source_is_gone() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("Name.svg");
        fs::write(&source, svg("red")).unwrap();
        assert!(render_schema_logo(dir.path(), "id", "Name", 192).is_some());
        let output = dir.path().join(".keytao-logo");
        fs::write(output.join("old-schema.png"), b"old").unwrap();
        fs::write(output.join(".skipped"), b"existing entry\n").unwrap();
        fs::remove_file(source).unwrap();
        assert!(render_schema_logo(dir.path(), "id", "Name", 192).is_none());
        assert!(!output.join("old-schema.png").exists());
        assert_eq!(
            fs::read_to_string(output.join(".skipped")).unwrap(),
            "existing entry\n"
        );
    }

    #[test]
    fn svg_and_ico_render_with_padding_and_clamped_size() {
        let dir = tempdir().unwrap();
        for (extension, bytes) in [("svg", svg("red")), ("ico", ico([255, 0, 0, 255]))] {
            let path = dir.path().join(format!("Name.{extension}"));
            fs::write(&path, bytes).unwrap();
            for (requested, size) in [(0, 16), (48, 48), (u32::MAX, 256)] {
                let bytes = render_schema_logo(dir.path(), "id", "Name", requested).unwrap();
                let image = png(&bytes, size);
                assert_eq!(image.get_pixel(size / 2, size / 2).0, [255, 0, 0, 255]);
                assert_eq!(image.get_pixel(size / 2, 0).0[3], 0);
            }
            fs::remove_file(path).unwrap();
        }
    }
}
