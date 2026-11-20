//! Check the actual Swift decoder field list without building a platform frontend.
use keytao_theme::{source::*, *};
use serde_json::{json, Value};

fn swift_fields(name: &str) -> Vec<(&'static str, &'static str)> {
    let swift = include_str!("../../keytao-macos-ime/Sources/KeyTaoIME/ImeTheme.swift");
    let declaration = format!("struct {name}: Codable {{");
    swift
        .split_once(&declaration)
        .unwrap()
        .1
        .split("\n    }")
        .next()
        .unwrap()
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("var "))
        .filter(|line| !line.contains('{'))
        .filter_map(|line| line.split_once(": "))
        .collect()
}

fn check_fields(name: &str, value: &Value) {
    for (field, kind) in swift_fields(name) {
        let v = value
            .get(field)
            .unwrap_or_else(|| panic!("Swift {name}.{field} missing"));
        if kind.ends_with('?') && v.is_null() {
            continue;
        }
        match kind.trim_end_matches('?') {
            "String"
            | "ThemeFontWeight"
            | "CandidatePanelOrientation"
            | "ThemeColorScheme"
            | "EffectiveThemeColorScheme" => assert!(v.is_string(), "{name}.{field}"),
            "CGFloat" | "TimeInterval" | "Int" => assert!(v.is_number(), "{name}.{field}"),
            "Bool" => assert!(v.is_boolean(), "{name}.{field}"),
            "ThemeColor" => {
                for component in ["red", "green", "blue", "alpha"] {
                    assert!(
                        v[component].as_u64().is_some_and(|n| n <= 255),
                        "{name}.{field}.{component}"
                    );
                }
            }
            "ShadowStyle" => check_fields("ShadowStyle", v),
            _ => panic!("Unrecognized Swift decoder type: {kind}"),
        }
    }
}

#[test]
fn compiled_json_round_trips_against_swift_decoder_fields() {
    for appearance in ["both", "light", "dark"] {
        let source = parse_theme_source_json(&json!({"version":3,"meta":{"id":"my.contract","name":"Contract","appearance":appearance},"common":{"accentColor":"#12345678","font":{"family":"A, B","weight":"heavy"}},"mobile":{"keyboard":{"key":{"background":"#12345678"}}}}).to_string()).unwrap();
        let compiled = compile_theme(&source, "").unwrap();
        let path = std::env::temp_dir().join(format!(
            "keytao-swift-contract-{}-{appearance}.yaml",
            std::process::id()
        ));
        std::fs::write(&path, compiled).unwrap();
        for scheme in [EffectiveColorScheme::Light, EffectiveColorScheme::Dark] {
            let theme = resolve_theme_from_paths_with_system_scheme(None, Some(&path), scheme);
            let encoded = resolved_renderer_theme_json(&theme).unwrap();
            let decoded: Value = serde_json::from_str(&encoded).unwrap();
            for (name, field) in [
                ("Ui", "ui"),
                ("Font", "font"),
                ("Panel", "panel"),
                ("CandidateOption", "candidate"),
                ("Navigation", "navigation"),
                ("ModeHint", "modeHint"),
            ] {
                check_fields(name, &decoded[field]);
            }
            assert_eq!(decoded["version"], 2);
            assert_eq!(
                decoded["mobile"],
                resolve_theme_source(&source, scheme).mobile
            );
            assert_eq!(
                decoded,
                serde_json::from_str::<Value>(&serde_json::to_string(&decoded).unwrap()).unwrap()
            );
        }
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn v3_preview_matches_saved_source_and_preserves_explicit_styles() {
    let source=parse_theme_source_json(&json!({"version":3,"meta":{"id":"my.preview","name":"Preview","appearance":"both"},"pc":{"candidate":{"selectedBackground":"#11223344","cornerRadius":3}},"mobile":{"keyboard":{"key":{"background":"#10203040"}}}}).to_string()).unwrap();
    let dir = std::env::temp_dir().join(format!("keytao-source-preview-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("themes")).unwrap();
    let path = dir.join("theme.yaml");
    let source_path = dir.join("themes/my.preview.yaml");
    let original = source.to_yaml().unwrap();
    std::fs::write(&source_path, &original).unwrap();
    std::fs::write(&path, compile_theme(&source, "").unwrap()).unwrap();
    let preview = resolve_candidate_panel_preview_from_paths(
        None,
        Some(&path),
        EffectiveColorScheme::Dark,
        UiColorScheme::Light,
        "#445566",
        24.0,
        Some(20.0),
        "#FFFFFF",
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&source_path).unwrap(), original);
    write_ime_ui_settings_to_path(
        &path,
        UiColorScheme::Light,
        PanelOrientation::Horizontal,
        Some("#445566".into()),
        24.0,
        Some(20.0),
        "#FFFFFF".into(),
    )
    .unwrap();
    let saved = scaled_candidate_panel(&resolve_theme_from_paths_with_system_scheme(
        None,
        Some(&path),
        EffectiveColorScheme::Dark,
    ));
    assert_eq!(
        resolved_theme_json(&preview).unwrap(),
        resolved_theme_json(&saved).unwrap()
    );
    assert_eq!(saved.candidate.selected_background.alpha, 0x44);
    assert_eq!(saved.candidate.corner_radius, 3.0);
    std::fs::remove_dir_all(dir).unwrap();
}
