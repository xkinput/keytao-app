//! Portable v3 theme exchange. These functions never read or write user files.
//!
//! App/bridge callers can export [`encode_theme_share`] or
//! [`canonical_source_json`], import with [`decode_theme_share`], and ask
//! [`resolve_import_conflict`] for the default, non-destructive install action.

use crate::source::{parse_theme_source_json, ThemeSource, THEME_SOURCE_VERSION};
pub use crate::source::{MAX_THEME_SOURCE_BYTES, MAX_THEME_SOURCE_DEPTH};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use flate2::{write::ZlibEncoder, Compression, Decompress, FlushDecompress, Status};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Write;
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

/// Above this many characters the App should offer file sharing instead.
pub const THEME_SHARE_FILE_THRESHOLD: usize = 4000;
/// Maximum cleaned share text accepted before base64 decoding.
pub const THEME_SHARE_MAX_TEXT_BYTES: usize = 96 * 1024;
const SHARE_PREFIX: &str = "keytao-theme:3:";

/// NFC + RFC 8785 JSON, including UTF-16 key order and ECMAScript numbers.
/// Unknown fields participate; ambiguous keys after NFC normalization fail.
pub fn canonical_json(value: &Value) -> Result<String, String> {
    fn write(value: &Value, out: &mut String, depth: usize) -> Result<(), String> {
        if depth > MAX_THEME_SOURCE_DEPTH {
            return Err("Theme source exceeds the nesting limit".into());
        }
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Value::Number(value) => {
                let number = value
                    .as_f64()
                    .ok_or("Theme number is outside the JCS range")?;
                if !number.is_finite() {
                    return Err("Theme number must be finite".into());
                }
                out.push_str(ryu_js::Buffer::new().format_finite(number));
            }
            Value::String(value) => {
                out.push_str(
                    &serde_json::to_string(&value.nfc().collect::<String>())
                        .map_err(|error| error.to_string())?,
                );
            }
            Value::Array(values) => {
                out.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        out.push(',');
                    }
                    write(value, out, depth + 1)?;
                }
                out.push(']');
            }
            Value::Object(values) => {
                let mut entries: Vec<_> = values
                    .iter()
                    .map(|(key, value)| (key.nfc().collect::<String>(), value))
                    .collect();
                entries
                    .sort_by(|(left, _), (right, _)| left.encode_utf16().cmp(right.encode_utf16()));
                if entries
                    .windows(2)
                    .any(|entries| entries[0].0 == entries[1].0)
                {
                    return Err("Theme keys collide after NFC normalization".into());
                }
                out.push('{');
                for (index, (key, value)) in entries.into_iter().enumerate() {
                    if index != 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::to_string(&key).map_err(|error| error.to_string())?);
                    out.push(':');
                    write(value, out, depth + 1)?;
                }
                out.push('}');
            }
        }
        if out.len() > MAX_THEME_SOURCE_BYTES {
            return Err("Theme source exceeds 64 KiB".into());
        }
        Ok(())
    }
    let mut output = String::new();
    write(value, &mut output, 0)?;
    Ok(output)
}

/// Canonical full source partial, preserving unknown fields for stable identity.
/// This is the input to the compiled theme's hash, without resolved defaults.
pub fn canonical_source_json(source: &ThemeSource) -> Result<String, String> {
    canonical_json(&source.value)
}

/// Full lowercase SHA-256 used for import identity; compilation takes 16 hex digits.
pub fn theme_source_hash(source: &ThemeSource) -> Result<String, String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(canonical_source_json(source)?.as_bytes())
    ))
}

fn portable_source(mut value: Value) -> Value {
    fn clean_local_fields(value: &mut Value) {
        match value {
            Value::Object(object) => {
                object.retain(|key, _| {
                    !matches!(
                        key.as_str(),
                        "ui" | "themeId"
                            | "themeHash"
                            | "compiledHash"
                            | "resolvedUi"
                            | "behavior"
                            | "localOnly"
                            | "local-only"
                            | "local_only"
                            | "localPath"
                            | "path"
                            | "paths"
                            | "sourcePath"
                            | "filePath"
                    ) && !key.ends_with("Path")
                        && !key.ends_with("Paths")
                        && !key.ends_with("_path")
                        && !key.ends_with("_paths")
                });
                for value in object.values_mut() {
                    clean_local_fields(value);
                }
            }
            Value::Array(array) => {
                for value in array {
                    clean_local_fields(value);
                }
            }
            _ => (),
        }
    }
    fn clean_variant(value: &mut Value) {
        let Some(root) = value.as_object_mut() else {
            return;
        };
        root.remove("keyboard");
        if let Some(keyboard) = root
            .get_mut("mobile")
            .and_then(|mobile| mobile.get_mut("keyboard"))
            .and_then(Value::as_object_mut)
        {
            for key in [
                "height",
                "horizontalGap",
                "verticalGap",
                "outerInset",
                "maxKeyHeight",
                "layout",
                "layouts",
                "behavior",
                "sound",
                "vibration",
                "keyBubble",
                "localPath",
                "path",
                "paths",
            ] {
                keyboard.remove(key);
            }
        }
        if let Some(bar) = root
            .get_mut("mobile")
            .and_then(|mobile| mobile.get_mut("candidateBar"))
            .and_then(Value::as_object_mut)
        {
            bar.remove("candidateBarHeight");
        }
    }
    clean_local_fields(&mut value);
    clean_variant(&mut value);
    for variant in ["light", "dark"] {
        if let Some(value) = value.get_mut(variant) {
            clean_variant(value);
        }
    }
    value
}

/// Canonical portable JSON file contents, excluding behavior, geometry and local state.
pub fn export_theme_source_json(source: &ThemeSource) -> Result<String, String> {
    let portable = canonical_json(&portable_source(source.value.clone()))?;
    canonical_source_json(&parse_theme_source_json(&portable)?)
}

/// Encode only the source partial as `keytao-theme:3:<base64url(zlib(JSON))>`.
pub fn encode_theme_share(source: &ThemeSource) -> Result<String, String> {
    let canonical = export_theme_source_json(source)?;
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(canonical.as_bytes())
        .map_err(|error| error.to_string())?;
    let compressed = encoder.finish().map_err(|error| error.to_string())?;
    Ok(format!(
        "{SHARE_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(compressed)
    ))
}

fn pasted_whitespace(character: char) -> bool {
    character.is_whitespace() || matches!(character, '\u{200b}'..='\u{200d}' | '\u{feff}')
}

fn parse_portable_theme(value: Value) -> Result<ThemeSource, String> {
    fn report_excluded(original: &Value, portable: &Value, path: &str, dropped: &mut Vec<String>) {
        match original {
            Value::Object(object) => {
                for (key, value) in object {
                    let full = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    if let Some(kept) = portable.get(key) {
                        report_excluded(value, kept, &full, dropped);
                    } else {
                        dropped.push(format!("{full}: excluded from theme sharing"));
                    }
                }
            }
            Value::Array(array) => {
                for (index, value) in array.iter().enumerate() {
                    if let Some(kept) = portable.get(index) {
                        report_excluded(value, kept, &format!("{path}[{index}]"), dropped);
                    }
                }
            }
            _ => (),
        }
    }
    let portable = portable_source(value.clone());
    let mut source = parse_theme_source_json(&canonical_json(&portable)?)?;
    report_excluded(&value, &portable, "", &mut source.report.dropped);
    Ok(source)
}

/// Decode a share string or plain JSON, without touching local IME settings.
/// Share whitespace/zero-width cleanup leaves spaces inside plain JSON intact.
/// Zlib must reach its checksum-verified end with no trailing compressed bytes.
pub fn decode_theme_share(text: &str) -> Result<ThemeSource, String> {
    let trimmed = text.trim_matches(pasted_whitespace);
    if trimmed.starts_with('{') {
        let mut cleaned = String::new();
        let (mut quoted, mut escaped) = (false, false);
        for character in trimmed.chars() {
            if matches!(character, '\u{200b}'..='\u{200d}' | '\u{feff}')
                || (!quoted && character.is_whitespace())
            {
                continue;
            }
            cleaned.push(character);
            if cleaned.len() > THEME_SHARE_MAX_TEXT_BYTES {
                return Err("Theme share text exceeds 96 KiB".into());
            }
            if escaped {
                escaped = false;
            } else if quoted && character == '\\' {
                escaped = true;
            } else if character == '"' {
                quoted = !quoted;
            }
        }
        let value: Value = serde_json::from_str(&cleaned).map_err(|error| error.to_string())?;
        return parse_portable_theme(value);
    }
    let mut cleaned = String::with_capacity(text.len().min(THEME_SHARE_MAX_TEXT_BYTES));
    for character in text
        .chars()
        .filter(|character| !pasted_whitespace(*character))
    {
        cleaned.push(character);
        if cleaned.len() > THEME_SHARE_MAX_TEXT_BYTES {
            return Err("Theme share text exceeds 96 KiB".into());
        }
    }
    let rest = cleaned
        .strip_prefix("keytao-theme:")
        .ok_or("Expected a theme share string or JSON")?;
    let (version, encoded) = rest
        .split_once(':')
        .ok_or("Theme share version is missing")?;
    let version: u32 = version.parse().map_err(|_| "Invalid theme share version")?;
    if version > THEME_SOURCE_VERSION {
        return Err("Theme requires a newer App version".into());
    }
    if version != THEME_SOURCE_VERSION {
        return Err("Only v3 theme shares are supported".into());
    }
    let compressed = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|error| error.to_string())?;
    let mut output = vec![0; MAX_THEME_SOURCE_BYTES + 1];
    let mut decoder = Decompress::new(true);
    let status = decoder
        .decompress(&compressed, &mut output, FlushDecompress::Finish)
        .map_err(|error| format!("Invalid theme zlib data: {error}"))?;
    if decoder.total_out() > MAX_THEME_SOURCE_BYTES as u64 {
        return Err("Theme source exceeds 64 KiB".into());
    }
    if status != Status::StreamEnd || decoder.total_in() != compressed.len() as u64 {
        return Err("Theme zlib data is truncated or has trailing bytes".into());
    }
    output.truncate(decoder.total_out() as usize);
    let value: Value = serde_json::from_slice(&output).map_err(|error| error.to_string())?;
    parse_portable_theme(value)
}

/// Install action selected without writing files. Built-in themes are read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeImportDisposition {
    Install,
    AlreadyInstalled,
    SaveAsCopy,
    Overwrite,
}

/// A source is returned only when a file should be installed or replaced.
pub struct ThemeImportResolution {
    pub disposition: ThemeImportDisposition,
    pub source: Option<ThemeSource>,
}

/// Resolve id conflicts. `overwrite = false` is the safe, save-as-copy default.
/// On `Overwrite`, an App replacing the active id must recompile immediately.
/// Built-ins fork into the `my.` namespace so the copy remains editable.
pub fn resolve_import_conflict(
    incoming: &ThemeSource,
    installed: &[ThemeSource],
    overwrite: bool,
) -> Result<ThemeImportResolution, String> {
    let portable = parse_theme_source_json(&export_theme_source_json(incoming)?)?;
    let existing = installed.iter().find(|source| source.id() == portable.id());
    if let Some(existing) = existing {
        let existing = parse_theme_source_json(&export_theme_source_json(existing)?)?;
        if theme_source_hash(&existing)? == theme_source_hash(&portable)? {
            return Ok(ThemeImportResolution {
                disposition: ThemeImportDisposition::AlreadyInstalled,
                source: None,
            });
        }
    }
    let builtin = portable.id().starts_with("keytao.");
    if existing.is_none() && !builtin {
        return Ok(ThemeImportResolution {
            disposition: ThemeImportDisposition::Install,
            source: Some(portable),
        });
    }
    if overwrite && !builtin {
        return Ok(ThemeImportResolution {
            disposition: ThemeImportDisposition::Overwrite,
            source: Some(portable),
        });
    }
    let base_id = if builtin {
        format!("my.{}", portable.id().trim_start_matches("keytao."))
    } else {
        portable.id().to_owned()
    };
    let mut suffix = 2_u64;
    let id = loop {
        let ending = format!("-{suffix}");
        let root = &base_id[..base_id.len().min(64 - ending.len())];
        let candidate = format!("{}{ending}", root.trim_end_matches(['.', '-', '_']));
        if !installed.iter().any(|source| source.id() == candidate) {
            break candidate;
        }
        suffix = suffix
            .checked_add(1)
            .ok_or("Theme copy id space exhausted")?;
    };
    let mut value = portable.value;
    let name = value["meta"]["name"].as_str().unwrap_or_default();
    let name = format!(
        "{}（副本）",
        name.graphemes(true).take(60).collect::<String>()
    );
    value["meta"]["id"] = Value::String(id);
    value["meta"]["name"] = Value::String(name);
    let source = parse_theme_source_json(&canonical_json(&value)?)?;
    Ok(ThemeImportResolution {
        disposition: ThemeImportDisposition::SaveAsCopy,
        source: Some(source),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn source(id: &str) -> ThemeSource {
        parse_theme_source_json(
            &json!({"version":3,"meta":{"id":id,"name":"Mint Theme"},
            "common":{"accentColor":"#2bb38a","font":{"family":["PingFang SC", "Noto Sans"]}}})
            .to_string(),
        )
        .unwrap()
    }

    fn encoded_raw(bytes: &[u8]) -> String {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        format!(
            "{SHARE_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(encoder.finish().unwrap())
        )
    }

    #[test]
    fn jcs_numbers_strings_nfc_and_utf16_order() {
        let value = json!({"\u{e000}":1,"😀":2,"a":"e\u{301}","n":[20.0,-0.0,1e-7,1e-6,1e20,1e21,333333333.33333329]});
        assert_eq!(canonical_json(&value).unwrap(),
            "{\"a\":\"é\",\"n\":[20,0,1e-7,0.000001,100000000000000000000,1e+21,333333333.3333333],\"😀\":2,\"\u{e000}\":1}");
        assert_eq!(
            canonical_json(&json!({"a":20})).unwrap(),
            canonical_json(&json!({"a":20.0})).unwrap()
        );
        assert!(canonical_json(&json!({"é":1,"e\u{301}":2})).is_err());
    }

    #[test]
    fn canonical_source_retains_explicit_defaults_and_unknown_fields() {
        let a = parse_theme_source_json(r##"{"version":3,"meta":{"id":"my.mint","name":"e\u0301"},"common":{"accentColor":"#abc","cornerRadius":16,"font":{"family":["PingFang SC","Noto Sans"]}},"futureStyle":{"x":20.0}}"##).unwrap();
        let b = parse_theme_source_json(r##"{"futureStyle":{"x":20},"common":{"cornerRadius":16,"accentColor":"#AABBCCFF","font":{"family":"  PingFang SC,  Noto Sans  "}},"meta":{"name":"é","id":"my.mint"},"version":3}"##).unwrap();
        assert_eq!(
            canonical_source_json(&a).unwrap(),
            canonical_source_json(&b).unwrap()
        );
        assert_eq!(
            theme_source_hash(&a).unwrap(),
            theme_source_hash(&b).unwrap()
        );
        assert!(canonical_source_json(&a).unwrap().contains("cornerRadius"));
        assert!(canonical_source_json(&a).unwrap().contains("futureStyle"));
    }

    #[test]
    fn share_roundtrip_normalizes_and_cleans_pasted_transport_text() {
        let original = source("my.mint");
        let encoded = encode_theme_share(&original).unwrap();
        let pasted = format!(
            "\u{feff}\u{200b} {} \n\t\u{200d}",
            encoded
                .chars()
                .map(|character| format!("{character}\u{200c} "))
                .collect::<String>()
        );
        let decoded = decode_theme_share(&pasted).unwrap();
        assert_eq!(
            canonical_source_json(&original).unwrap(),
            canonical_source_json(&decoded).unwrap()
        );
        let plain = decode_theme_share(" \u{feff} {\"version\":3,\"meta\":{\"id\":\"my.plain\",\"name\":\"Mint Theme\"},\"common\":{\"font\":{\"family\":\"PingFang SC, Noto Sans\"}}} ").unwrap();
        assert_eq!(plain.value["meta"]["name"], "Mint Theme");
        assert_eq!(
            plain.value["common"]["font"]["family"],
            "PingFang SC, Noto Sans"
        );
        assert!(decode_theme_share("version: 3\nmeta: {id: my.yaml, name: YAML}").is_err());
    }

    #[test]
    fn legacy_local_text_is_normalized_at_the_share_boundary() {
        let local = crate::source::parse_theme_source_yaml("version: 3\nmeta: {id: my.legacy, name: Legacy}\ncommon: {font: {family: ' A ,B,  C D '}}\npc: {candidate: {labelSuffix: '12345678'}}\n").unwrap();
        let exported = export_theme_source_json(&local).unwrap();
        let decoded = decode_theme_share(&encode_theme_share(&local).unwrap()).unwrap();
        assert_eq!(exported, canonical_source_json(&decoded).unwrap());
        assert_eq!(decoded.value["common"]["font"]["family"], "A, B, C D");
        assert_eq!(decoded.value["pc"]["candidate"]["labelSuffix"], "1234");
        assert_eq!(local.value["pc"]["candidate"]["labelSuffix"], "12345678");
        let resolution = resolve_import_conflict(&decoded, &[local], false).unwrap();
        assert_eq!(
            resolution.disposition,
            ThemeImportDisposition::AlreadyInstalled
        );
    }

    #[test]
    fn share_excludes_behavior_local_state_and_geometry_without_losing_style() {
        let original = parse_theme_source_json(r##"{"version":3,"meta":{"id":"my.mint","name":"Mint"},"ui":{"colorScheme":"dark"},"themeId":"local","themeHash":"secret","resolvedUi":{"accentColor":"#123"},"keyboard":{"rows":[]},"behavior":{"sound":true},"localPath":"/local/private","mobile":{"keyboard":{"height":300,"horizontalGap":5,"key":{"background":"#ABC"},"keyTypes":{"space":{"foreground":"#123"}}}},"light":{"resolvedUi":{"themeHash":"light-local"}},"dark":{"ui":{"colorScheme":"light"},"resolvedUi":{"themeHash":"dark-local"},"mobile":{"keyboard":{"height":400,"background":"#123"}}},"futureStyle":{"color":"#456","nested":[{"assetPath":"/local/private","font_paths":["/local/font"],"local_only":{"private":"local"},"behavior":{"sound":true},"paths":["/local/path"],"otherStyle":{"amount":2}}]}}"##).unwrap();
        let decoded = decode_theme_share(&encode_theme_share(&original).unwrap()).unwrap();
        for key in [
            "ui",
            "themeId",
            "themeHash",
            "resolvedUi",
            "keyboard",
            "behavior",
            "localPath",
        ] {
            assert!(decoded.value.get(key).is_none(), "{key}");
        }
        assert!(decoded.value["mobile"]["keyboard"].get("height").is_none());
        assert!(decoded.value["mobile"]["keyboard"]
            .get("horizontalGap")
            .is_none());
        assert_eq!(
            decoded.value["mobile"]["keyboard"]["key"]["background"],
            "#AABBCCFF"
        );
        assert_eq!(
            decoded.value["mobile"]["keyboard"]["keyTypes"]["space"]["foreground"],
            "#112233FF"
        );
        assert!(decoded.value["dark"].get("ui").is_none());
        assert!(decoded.value["light"].get("resolvedUi").is_none());
        assert!(decoded.value["dark"].get("resolvedUi").is_none());
        assert_eq!(decoded.value["futureStyle"]["color"], "#456");
        assert_eq!(
            decoded.value["futureStyle"]["nested"][0],
            json!({"otherStyle":{"amount":2}})
        );
        assert_eq!(
            decoded.value["dark"]["mobile"]["keyboard"]["background"],
            "#112233FF"
        );
    }

    #[test]
    fn imports_report_excluded_fields_alongside_validation_diagnostics() {
        let value = json!({"version":3,"meta":{"id":"my.report","name":"Report"},
            "ui":{"colorScheme":"dark"},"localPath":"/private/local",
            "common":{"cornerRadius":100},
            "mobile":{"keyboard":{"height":300,"key":{"background":"bad","foreground":"#123"}}}});
        let json = value.to_string();
        for text in [json.clone(), encoded_raw(json.as_bytes())] {
            let source = decode_theme_share(&text).unwrap();
            for path in ["ui", "localPath", "mobile.keyboard.height"] {
                assert!(source.report.dropped.iter().any(
                    |diagnostic| diagnostic == &format!("{path}: excluded from theme sharing")
                ));
            }
            assert!(source
                .report
                .dropped
                .iter()
                .any(|diagnostic| diagnostic == "mobile.keyboard.key.background"));
            assert!(source
                .report
                .clamped
                .iter()
                .any(|diagnostic| diagnostic.contains("common.cornerRadius")));
            assert_eq!(
                source.value["mobile"]["keyboard"]["key"]["foreground"],
                "#112233FF"
            );
        }
    }

    #[test]
    fn transport_limits_apply_after_cleaning_and_before_decompression() {
        let original = source("my.transport");
        let encoded = encode_theme_share(&original).unwrap();
        let padded = encoded
            .chars()
            .map(|character| format!("{character}{}\u{200b}", " ".repeat(1024)))
            .collect::<String>();
        assert!(padded.len() > THEME_SHARE_MAX_TEXT_BYTES);
        assert_eq!(
            theme_source_hash(&decode_theme_share(&padded).unwrap()).unwrap(),
            theme_source_hash(&original).unwrap()
        );

        let compressed = vec![0; THEME_SHARE_MAX_TEXT_BYTES * 3 / 4];
        let oversized = format!("{SHARE_PREFIX}{}", URL_SAFE_NO_PAD.encode(compressed));
        let pasted = oversized
            .chars()
            .map(|character| format!("\u{200d}{character}\u{a0}"))
            .collect::<String>();
        assert!(decode_theme_share(&pasted).unwrap_err().contains("96 KiB"));
        let at_limit = format!(
            "{SHARE_PREFIX}{}",
            "A".repeat(THEME_SHARE_MAX_TEXT_BYTES - SHARE_PREFIX.len())
        );
        assert_eq!(at_limit.len(), THEME_SHARE_MAX_TEXT_BYTES);
        assert!(!decode_theme_share(&at_limit)
            .unwrap_err()
            .contains("96 KiB"));

        let mut value =
            json!({"version":3,"meta":{"id":"my.boundary","name":"Boundary"},"future":""});
        let overhead = value.to_string().len();
        value["future"] = json!("x".repeat(MAX_THEME_SOURCE_BYTES - overhead));
        let at_limit = value.to_string();
        assert_eq!(at_limit.len(), MAX_THEME_SOURCE_BYTES);
        assert!(decode_theme_share(&encoded_raw(at_limit.as_bytes())).is_ok());
        value["future"] = json!("x".repeat(MAX_THEME_SOURCE_BYTES - overhead + 1));
        let bomb = encoded_raw(value.to_string().as_bytes());
        assert!(bomb.len() < 1024);
        assert!(decode_theme_share(&bomb).unwrap_err().contains("64 KiB"));
        let pasted="\u{feff}{\u{200b}\u{a0}\"version\":3,\"meta\":{\"id\":\"my.pasted\",\"name\":\"A B \\\"C\\\"\"}}";
        assert_eq!(decode_theme_share(pasted).unwrap().name(), "A B \"C\"");
    }

    #[test]
    fn share_caps_versions_and_corruption_are_rejected() {
        assert_eq!(THEME_SHARE_FILE_THRESHOLD, 4000);
        assert!(
            decode_theme_share(&format!("keytao-theme:4:{}", "!invalid!"))
                .unwrap_err()
                .contains("newer")
        );
        assert!(decode_theme_share("keytao-theme:2:AAAA").is_err());
        assert!(decode_theme_share(&"a".repeat(THEME_SHARE_MAX_TEXT_BYTES + 1)).is_err());
        assert!(decode_theme_share(&encoded_raw(&vec![b' '; MAX_THEME_SOURCE_BYTES + 1])).is_err());
        assert!(canonical_json(&json!({"unknown":"x".repeat(MAX_THEME_SOURCE_BYTES)})).is_err());
        let mut nested = Value::Null;
        for _ in 0..=MAX_THEME_SOURCE_DEPTH {
            nested = json!({"x":nested});
        }
        assert!(canonical_json(&nested).is_err());
        let valid = encode_theme_share(&source("my.mint")).unwrap();
        let compressed = URL_SAFE_NO_PAD
            .decode(valid.strip_prefix(SHARE_PREFIX).unwrap())
            .unwrap();
        for length in [0, 1, compressed.len() - 1, compressed.len() - 4] {
            assert!(decode_theme_share(&format!(
                "{SHARE_PREFIX}{}",
                URL_SAFE_NO_PAD.encode(&compressed[..length])
            ))
            .is_err());
        }
        let mut corrupt = compressed.clone();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
        assert!(decode_theme_share(&format!(
            "{SHARE_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(corrupt)
        ))
        .is_err());
        let mut trailing = compressed;
        trailing.push(0);
        assert!(decode_theme_share(&format!(
            "{SHARE_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(trailing)
        ))
        .is_err());
    }

    #[test]
    fn conflicts_default_to_copies_identical_is_noop_and_builtins_are_readonly() {
        let incoming = source("my.mint");
        assert_eq!(
            resolve_import_conflict(&incoming, &[], false)
                .unwrap()
                .disposition,
            ThemeImportDisposition::Install
        );
        assert_eq!(
            resolve_import_conflict(&incoming, &[source("my.mint")], false)
                .unwrap()
                .disposition,
            ThemeImportDisposition::AlreadyInstalled
        );
        let old =
            parse_theme_source_json(r#"{"version":3,"meta":{"id":"my.mint","name":"Old Mint"}}"#)
                .unwrap();
        let copied =
            resolve_import_conflict(&incoming, &[old, source("my.mint-2")], false).unwrap();
        assert_eq!(copied.disposition, ThemeImportDisposition::SaveAsCopy);
        let copied = copied.source.unwrap();
        assert_eq!(copied.id(), "my.mint-3");
        assert_eq!(copied.value["meta"]["name"], "Mint Theme（副本）");
        let old =
            parse_theme_source_json(r#"{"version":3,"meta":{"id":"my.mint","name":"Old Mint"}}"#)
                .unwrap();
        assert_eq!(
            resolve_import_conflict(&incoming, &[old], true)
                .unwrap()
                .disposition,
            ThemeImportDisposition::Overwrite
        );
        let builtin = resolve_import_conflict(&source("keytao.mint"), &[], true).unwrap();
        assert_eq!(builtin.disposition, ThemeImportDisposition::SaveAsCopy);
        assert_eq!(builtin.source.unwrap().id(), "my.mint-2");
    }
}
