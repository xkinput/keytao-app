use keytao_theme::{
    resolved_renderer_theme_json, resolved_theme_json, source::*, EffectiveColorScheme,
};
use serde_json::Value;

#[test]
fn default_resolved_json_matches_renderer_contract_in_both_appearances() {
    let source = parse_theme_source_json(
        r#"{"version":3,"meta":{"id":"keytao.default","name":"Default","appearance":"both"}}"#,
    )
    .unwrap();
    for (scheme, golden) in [
        (
            EffectiveColorScheme::Light,
            include_str!("fixtures/default-light.json"),
        ),
        (
            EffectiveColorScheme::Dark,
            include_str!("fixtures/default-dark.json"),
        ),
    ] {
        let actual: Value = serde_json::from_str(
            &resolved_renderer_theme_json(&resolve_theme_source(&source, scheme)).unwrap(),
        )
        .unwrap();
        let expected: Value = serde_json::from_str(golden).unwrap();
        assert_eq!(
            actual, expected,
            "{scheme:?} renderer fields or defaults changed"
        );
    }
}

#[test]
fn emergency_renderer_fallbacks_match_rust_defaults() {
    let expected: Value =
        serde_json::from_str(include_str!("fixtures/default-light.json")).unwrap();
    for asset in [
        include_str!("../../../flutter_app/android/app/src/main/assets/keytao-default-theme.json"),
        include_str!(
            "../../keytao-ios-ime/Sources/KeyTaoIOSIME/Resources/default-theme-light.json"
        ),
    ] {
        assert_eq!(serde_json::from_str::<Value>(asset).unwrap(), expected);
    }
    let swift = include_str!("../../keytao-macos-ime/Sources/KeyTaoIME/ImeTheme.swift");
    let fallback = swift
        .split_once("private static let fallbackJSON = #\"")
        .unwrap()
        .1
        .split_once("\"#")
        .unwrap()
        .0;
    assert_eq!(serde_json::from_str::<Value>(fallback).unwrap(), expected);
}

#[test]
fn editor_and_native_candidate_metrics_use_the_same_rust_scale() {
    let source_json =
        r#"{"version":3,"meta":{"id":"my.scaled","name":"Scaled"},"pc":{"font":{"size":30}}}"#;
    let source = parse_theme_source_json(source_json).unwrap();
    let theme = resolve_theme_source(&source, EffectiveColorScheme::Light);
    let preview: Value = serde_json::from_str(
        &keytao_theme::editor::resolve_theme_editor_preview(source_json, false).unwrap(),
    )
    .unwrap();
    let native: Value = serde_json::from_str(
        &resolved_renderer_theme_json(&keytao_theme::scaled_candidate_panel(&theme)).unwrap(),
    )
    .unwrap();
    assert_eq!(preview, native);
    assert_eq!(preview["panel"]["paddingX"], 15.0);
    assert_eq!(preview["mobile"], theme.mobile);
    let legacy: Value = serde_json::from_str(&resolved_theme_json(&theme).unwrap()).unwrap();
    assert!(legacy["candidate"].get("preeditCaretColor").is_none());
    assert!(legacy["panel"].get("shadowStyle").is_none());
    assert!(legacy["modeHint"].get("shadowStyle").is_none());
}
