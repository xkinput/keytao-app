//! Editor field capabilities and validated source edits.
use crate::{source::ThemeSource, EffectiveColorScheme};
use serde::Serialize;
use serde_json::{json, Value};

const MAC: u8 = 1;
const WINDOWS: u8 = 2;
const LINUX: u8 = 4;
const MOBILE: u8 = 8;
const DESKTOP: u8 = MAC | WINDOWS | LINUX;
const ALL: u8 = DESKTOP | MOBILE;

// Keep editor availability and the editable path boundary in one table.
const FIELDS: &[(&str, u8)] = &[
    ("common.accentColor", ALL),
    ("common.backgroundColor", ALL),
    ("common.cornerRadius", ALL),
    ("common.font.family", ALL),
    ("common.font.weight", MAC | MOBILE),
    ("pc.panel.orientation", DESKTOP),
    ("pc.panel.background", DESKTOP),
    ("pc.panel.borderColor", DESKTOP),
    ("pc.panel.borderWidth", DESKTOP),
    ("pc.panel.cornerRadius", DESKTOP),
    ("pc.panel.paddingX", DESKTOP),
    ("pc.panel.paddingY", DESKTOP),
    ("pc.panel.paddingTop", DESKTOP),
    ("pc.panel.paddingRight", DESKTOP),
    ("pc.panel.paddingBottom", DESKTOP),
    ("pc.panel.paddingLeft", DESKTOP),
    ("pc.panel.gap", DESKTOP),
    ("pc.panel.preeditGap", DESKTOP),
    ("pc.panel.shadow", DESKTOP),
    ("pc.font.size", DESKTOP),
    ("pc.font.labelSize", DESKTOP),
    ("pc.font.commentSize", DESKTOP),
    ("pc.font.preeditSize", DESKTOP),
    ("pc.candidate.background", DESKTOP),
    ("pc.candidate.foreground", DESKTOP),
    ("pc.candidate.selectedForeground", DESKTOP),
    ("pc.candidate.selectedBackground", DESKTOP),
    ("pc.candidate.labelColor", DESKTOP),
    ("pc.candidate.selectedLabelColor", DESKTOP),
    ("pc.candidate.commentColor", DESKTOP),
    ("pc.candidate.commentPosition", DESKTOP),
    ("pc.candidate.selectedCommentColor", DESKTOP),
    ("pc.candidate.hoverBackground", MAC | WINDOWS),
    ("pc.candidate.borderColor", DESKTOP),
    ("pc.candidate.selectedBorderColor", DESKTOP),
    ("pc.candidate.borderWidth", DESKTOP),
    ("pc.candidate.cornerRadius", DESKTOP),
    ("pc.candidate.labelSuffix", DESKTOP),
    ("pc.candidate.verticalFullWidth", DESKTOP),
    ("pc.modeHint.background", DESKTOP),
    ("pc.modeHint.foreground", DESKTOP),
    ("pc.modeHint.borderColor", DESKTOP),
    ("pc.modeHint.borderWidth", DESKTOP),
    ("pc.modeHint.fontSize", DESKTOP),
    ("pc.modeHint.width", DESKTOP),
    ("pc.modeHint.height", DESKTOP),
    ("pc.modeHint.cornerRadius", DESKTOP),
    ("pc.modeHint.shadow", DESKTOP),
    ("mobile.keyboard.background", MOBILE),
    ("mobile.keyboard.key.background", MOBILE),
    ("mobile.keyboard.key.foreground", MOBILE),
    ("mobile.keyboard.key.hintForeground", MOBILE),
    ("mobile.keyboard.key.cornerRadius", MOBILE),
    ("mobile.keyboard.key.borderColor", MOBILE),
    ("mobile.keyboard.key.borderWidth", MOBILE),
    ("mobile.keyboard.key.shadowColor", MOBILE),
    ("mobile.keyboard.key.shadowOffsetY", MOBILE),
    ("mobile.keyboard.key.fontScale", MOBILE),
    ("mobile.keyboard.key.hintFontScale", MOBILE),
    ("mobile.keyboard.keyTypes.function.background", MOBILE),
    ("mobile.keyboard.keyTypes.function.foreground", MOBILE),
    ("mobile.keyboard.keyTypes.function.borderColor", MOBILE),
    ("mobile.keyboard.keyTypes.function.borderWidth", MOBILE),
    ("mobile.keyboard.keyTypes.space.background", MOBILE),
    ("mobile.keyboard.keyTypes.space.foreground", MOBILE),
    ("mobile.keyboard.keyTypes.space.borderColor", MOBILE),
    ("mobile.keyboard.keyTypes.space.borderWidth", MOBILE),
    ("mobile.keyboard.keyTypes.enter.background", MOBILE),
    ("mobile.keyboard.keyTypes.enter.foreground", MOBILE),
    ("mobile.keyboard.keyTypes.enter.borderColor", MOBILE),
    ("mobile.keyboard.keyTypes.enter.borderWidth", MOBILE),
    ("mobile.keyboard.keyClasses.accent.background", MOBILE),
    ("mobile.keyboard.keyClasses.accent.foreground", MOBILE),
    ("mobile.keyboard.keyClasses.accent.borderColor", MOBILE),
    ("mobile.keyboard.keyClasses.accent.borderWidth", MOBILE),
    ("mobile.keyboard.keyClasses.accentPunctuation.background", MOBILE),
    ("mobile.keyboard.keyClasses.accentPunctuation.foreground", MOBILE),
    ("mobile.keyboard.keyClasses.accentPunctuation.borderColor", MOBILE),
    ("mobile.keyboard.keyClasses.accentPunctuation.borderWidth", MOBILE),
    ("mobile.keyboard.toolbar.background", MOBILE),
    ("mobile.keyboard.toolbar.foreground", MOBILE),
    ("mobile.keyboard.toolbar.selectedForeground", MOBILE),
    ("mobile.keyboard.toolbar.buttonBackground", MOBILE),
    ("mobile.keyboard.toolbar.buttonBorderColor", MOBILE),
    ("mobile.keyboard.toolbar.activeBackground", MOBILE),
    ("mobile.keyboard.toolbar.selectedBackground", MOBILE),
    ("mobile.keyboard.toolbar.pressedBackground", MOBILE),
    ("mobile.keyboard.toolbar.activeBorderColor", MOBILE),
    ("mobile.keyboard.toolbar.buttonBorderWidth", MOBILE),
    ("mobile.keyboard.toolbar.buttonCornerRadius", MOBILE),
    ("mobile.candidateBar.fontScale", MOBILE),
    ("mobile.candidateBar.commentFontSize", MOBILE),
    ("mobile.candidateBar.showComment", MOBILE),
    ("mobile.candidateBar.commentPosition", MOBILE),
    ("mobile.candidateBar.background", MOBILE),
    ("mobile.candidateBar.itemBackground", MOBILE),
    ("mobile.candidateBar.foreground", MOBILE),
    ("mobile.candidateBar.selectedForeground", MOBILE),
    ("mobile.candidateBar.selectedBackground", MOBILE),
    ("mobile.candidateBar.borderColor", MOBILE),
    ("mobile.candidateBar.selectedBorderColor", MOBILE),
    ("mobile.candidateBar.labelColor", MOBILE),
    ("mobile.candidateBar.selectedLabelColor", MOBILE),
    ("mobile.candidateBar.commentColor", MOBILE),
    ("mobile.candidateBar.selectedCommentColor", MOBILE),
    ("mobile.candidateBar.borderWidth", MOBILE),
    ("mobile.candidateBar.selectedBorderWidth", MOBILE),
    ("mobile.candidateBar.cornerRadius", MOBILE),
    ("mobile.candidateBar.paddingX", MOBILE),
    ("mobile.candidateBar.inlineGap", MOBILE),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorPlatform {
    Macos,
    Windows,
    Linux,
    LinuxKimpanel,
    Android,
    Ios,
    Unsupported,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorCapabilities {
    pub platform: &'static str,
    pub supported_fields: Vec<&'static str>,
}

pub fn editor_capabilities(platform: EditorPlatform) -> EditorCapabilities {
    let (name, mask) = match platform {
        EditorPlatform::Macos => ("macos", MAC),
        EditorPlatform::Windows => ("windows", WINDOWS),
        EditorPlatform::Linux => ("linux", LINUX),
        EditorPlatform::LinuxKimpanel => ("linuxKimpanel", 0),
        EditorPlatform::Android => ("android", MOBILE),
        EditorPlatform::Ios => ("ios", MOBILE),
        EditorPlatform::Unsupported => ("unsupported", 0),
    };
    EditorCapabilities {
        platform: name,
        supported_fields: FIELDS
            .iter()
            .filter_map(|(path, supported)| (supported & mask != 0).then_some(*path))
            .collect(),
    }
}

/// Validate incoming editor values so edits preserve unrelated local source text.
pub fn edit_theme_source_field(
    source: &ThemeSource,
    target: &str,
    path: &str,
    value: Option<Value>,
) -> Result<ThemeSource, String> {
    if !matches!(target, "light" | "dark") {
        return Err("Theme edit target must be light or dark".into());
    }
    if path == "common.font" {
        let font = value
            .as_ref()
            .and_then(Value::as_object)
            .filter(|font| {
                font.len() == 2 && font.contains_key("family") && font.contains_key("weight")
            })
            .ok_or("Font edit requires family and weight")?;
        let next = edit_theme_source_field(
            source,
            target,
            "common.font.family",
            Some(font["family"].clone()),
        )?;
        return edit_theme_source_field(
            &next,
            target,
            "common.font.weight",
            Some(font["weight"].clone()),
        );
    }
    if path == "pc.panel.padding" {
        let fields = value
            .as_ref()
            .and_then(Value::as_object)
            .filter(|fields| !fields.is_empty())
            .ok_or("Panel padding edit requires at least one side")?;
        let mut next = source.clone();
        for (field, value) in fields {
            let side_path = match field.as_str() {
                "paddingTop" => "pc.panel.paddingTop",
                "paddingBottom" => "pc.panel.paddingBottom",
                "paddingLeft" => "pc.panel.paddingLeft",
                "paddingRight" => "pc.panel.paddingRight",
                _ => return Err("Unsupported panel padding side".into()),
            };
            next = edit_theme_source_field(
                &next,
                target,
                side_path,
                (!value.is_null()).then(|| value.clone()),
            )?;
        }
        return Ok(next);
    }
    if !FIELDS.iter().any(|(field, _)| *field == path) {
        return Err("Unsupported theme editor field".into());
    }
    let parts: Vec<_> = path.split('.').collect();
    let mut next = source.value.clone();
    if let Some(value) = value {
        let mut incoming = json!({"version":3,"meta":{"id":"my.validate","name":"Validate"}});
        set_field(&mut incoming, &parts, value);
        let incoming = crate::source::parse_theme_source_json(
            &serde_json::to_string(&incoming).map_err(|error| error.to_string())?,
        )?;
        let pointer = format!("/{}", parts.join("/"));
        let value = incoming
            .value
            .pointer(&pointer)
            .ok_or("Invalid theme field value")?;
        set_field(&mut next[target], &parts, value.clone());
    } else {
        remove_field(&mut next, &parts);
        if let Some(layer) = next.get_mut(target) {
            remove_field(layer, &parts);
            if layer.as_object().is_some_and(serde_json::Map::is_empty) {
                next.as_object_mut().unwrap().remove(target);
            }
        }
    }
    crate::source::parse_local_theme_source(next)
}

fn set_field(root: &mut Value, path: &[&str], value: Value) {
    let mut parent = root;
    for key in path {
        if !parent.is_object() {
            *parent = json!({});
        }
        parent = &mut parent[*key];
    }
    *parent = value;
}

fn remove_field(root: &mut Value, path: &[&str]) {
    let Some(object) = root.as_object_mut() else {
        return;
    };
    if path.len() == 1 {
        object.remove(path[0]);
    } else if let Some(child) = object.get_mut(path[0]) {
        remove_field(child, &path[1..]);
        if child.as_object().is_some_and(serde_json::Map::is_empty) {
            object.remove(path[0]);
        }
    }
}

pub fn resolve_theme_editor_preview(source_json: &str, dark: bool) -> Result<String, String> {
    if source_json.len() > crate::source::MAX_THEME_SOURCE_BYTES {
        return Err("Theme source exceeds 64 KiB".into());
    }
    let source = crate::source::parse_local_theme_source(
        serde_json::from_str(source_json).map_err(|error| error.to_string())?,
    )?;
    let scheme = if dark {
        EffectiveColorScheme::Dark
    } else {
        EffectiveColorScheme::Light
    };
    crate::resolved_renderer_theme_json(&crate::scaled_candidate_panel(
        &crate::source::resolve_theme_source(&source, scheme),
    ))
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> ThemeSource {
        crate::source::parse_theme_source_json(
            r##"{"version":3,"meta":{"id":"my.editor","name":"Editor"},"common":{"accentColor":"#123456"},"extension":{"keep":true}}"##,
        ).unwrap()
    }

    #[test]
    fn platform_tables_match_supported_rendering_fields() {
        let supported = |platform| editor_capabilities(platform).supported_fields;
        let mac = supported(EditorPlatform::Macos);
        assert!(mac.contains(&"common.font.family"));
        assert!(mac.contains(&"pc.panel.shadow"));
        assert!(!mac.iter().any(|path| path.starts_with("mobile.")));
        for platform in [EditorPlatform::Windows, EditorPlatform::Linux] {
            let fields = supported(platform);
            assert!(fields.contains(&"pc.panel.orientation"));
            assert!(fields.contains(&"pc.panel.shadow"));
            assert!(fields.contains(&"pc.modeHint.shadow"));
            assert_eq!(
                fields.contains(&"pc.candidate.hoverBackground"),
                platform == EditorPlatform::Windows
            );
            assert!(fields.contains(&"common.font.family"));
            assert!(!fields.contains(&"common.font.weight"));
        }
        assert!(supported(EditorPlatform::LinuxKimpanel).is_empty());
        let mobile = supported(EditorPlatform::Android);
        assert_eq!(mobile, supported(EditorPlatform::Ios));
        assert!(mobile.contains(&"mobile.keyboard.key.shadowColor"));
        assert!(mobile.contains(&"common.font.weight"));
        assert!(mobile.contains(&"common.font.family"));
        assert!(mobile.contains(&"mobile.candidateBar.showComment"));
        assert!(!mobile.iter().any(|path| path.starts_with("pc.")));
        let mut unique = std::collections::HashSet::new();
        for (path, _) in FIELDS {
            assert!(unique.insert(path));
            assert!(!path.starts_with("pc.") || !path.contains("pressed"));
            assert!(!path.contains("maxWidth"));
        }
    }

    fn field_value(path: &str) -> Value {
        match path.rsplit('.').next().unwrap() {
            "family" => json!("A, B"),
            "weight" => json!("bold"),
            "orientation" => json!("horizontal"),
            "commentPosition" => json!("below"),
            "shadow" | "showComment" => json!(false),
            "verticalFullWidth" => json!(true),
            "labelSuffix" => json!(":"),
            "fontScale" | "hintFontScale" => json!(1.1),
            "size" | "labelSize" | "commentSize" | "commentFontSize" | "preeditSize" | "fontSize" => {
                json!(21)
            }
            "width" => json!(60),
            "height" => json!(40),
            "cornerRadius" | "buttonCornerRadius" | "paddingX" | "paddingY" | "paddingTop"
            | "paddingRight" | "paddingBottom" | "paddingLeft" | "gap" | "inlineGap"
            | "preeditGap" => json!(7),
            "borderWidth" | "selectedBorderWidth" | "buttonBorderWidth" | "shadowOffsetY" => {
                json!(1)
            }
            _ => json!("#98765432"),
        }
    }

    #[test]
    fn every_editor_field_writes_and_removes_in_each_variant() {
        for target in ["light", "dark"] {
            for (path, _) in FIELDS {
                let original = source();
                let edited =
                    edit_theme_source_field(&original, target, path, Some(field_value(path)))
                        .unwrap();
                let pointer = format!("/{target}/{}", path.replace('.', "/"));
                assert!(edited.value.pointer(&pointer).is_some(), "{pointer}");
                let other = if target == "light" { "dark" } else { "light" };
                assert_eq!(edited.value.get(other), original.value.get(other));
                assert_eq!(edited.value["common"], original.value["common"]);
                assert_eq!(edited.value["extension"], original.value["extension"]);
                let roundtrip = edit_theme_source_field(&edited, target, path, None).unwrap();
                assert!(roundtrip.value.pointer(&pointer).is_none());
                let mut expected = original.value;
                if *path == "common.accentColor" {
                    expected.as_object_mut().unwrap().remove("common");
                }
                assert_eq!(roundtrip.value, expected);
            }
        }
    }

    #[test]
    fn reset_removes_base_and_target_fields_but_preserves_other_values() {
        let defaults = crate::source::parse_theme_source_json(
            r#"{"version":3,"meta":{"id":"my.defaults","name":"Defaults"}}"#,
        )
        .unwrap();
        for target in ["light", "dark"] {
            let other = if target == "light" { "dark" } else { "light" };
            for (base, variant) in [(true, false), (false, true), (true, true), (false, false)] {
                let mut value = json!({
                    "version": 3,
                    "meta": {"id": "my.reset", "name": "Reset", "appearance": "both"},
                    "pc": {"panel": {"paddingY": 12}},
                    target: {"pc": {"panel": {"paddingY": 13}}},
                    other: {"pc": {"panel": {"paddingX": 23}}},
                    "extension": {"keep": true}
                });
                if base {
                    value["pc"]["panel"]["paddingX"] = json!(20);
                }
                if variant {
                    value[target]["pc"]["panel"]["paddingX"] = json!(21);
                }
                let original = crate::source::parse_local_theme_source(value).unwrap();
                let reset =
                    edit_theme_source_field(&original, target, "pc.panel.paddingX", None).unwrap();
                assert!(reset.value.pointer("/pc/panel/paddingX").is_none());
                assert!(reset
                    .value
                    .pointer(&format!("/{target}/pc/panel/paddingX"))
                    .is_none());
                assert_eq!(reset.value["pc"]["panel"]["paddingY"], 12.0);
                assert_eq!(reset.value[target]["pc"]["panel"]["paddingY"], 13.0);
                assert_eq!(reset.value[other], original.value[other]);
                assert_eq!(reset.value["extension"], original.value["extension"]);
                let scheme = if target == "light" {
                    EffectiveColorScheme::Light
                } else {
                    EffectiveColorScheme::Dark
                };
                assert_eq!(
                    crate::source::resolve_theme_source(&reset, scheme)
                        .panel
                        .padding_x,
                    crate::source::resolve_theme_source(&defaults, scheme)
                        .panel
                        .padding_x
                );
            }
        }
    }

    #[test]
    fn edit_boundary_rejects_unknown_paths_and_invalid_values_without_losing_source() {
        let original = source();
        for path in [
            "meta.id",
            "ui.colorScheme",
            "pc.panel.maxWidth",
            "../theme.yaml",
            "mobile.keyboard.keyClasses.custom.background",
        ] {
            assert!(
                edit_theme_source_field(&original, "light", path, Some(json!("#FFFFFF"))).is_err()
            );
        }
        assert!(edit_theme_source_field(&original, "base", "common.accentColor", None).is_err());
        for value in [json!(null), json!("bad"), json!(42), json!({"red":1})] {
            assert!(
                edit_theme_source_field(&original, "light", "common.accentColor", Some(value))
                    .is_err()
            );
        }
        let clamped = edit_theme_source_field(
            &original,
            "dark",
            "mobile.keyboard.key.fontScale",
            Some(json!(9)),
        )
        .unwrap();
        assert_eq!(
            clamped.value["dark"]["mobile"]["keyboard"]["key"]["fontScale"],
            1.3
        );
        let preview =
            resolve_theme_editor_preview(&serde_json::to_string(&clamped.value).unwrap(), true)
                .unwrap();
        let preview: Value = serde_json::from_str(&preview).unwrap();
        assert_eq!(preview["mobile"]["isDark"], true);
        assert_eq!(preview["mobile"]["keyboard"]["key"]["fontScale"], 1.3);
    }

    #[test]
    fn font_pair_updates_both_leaves_and_preserves_other_source_fields() {
        for target in ["light", "dark"] {
            let other = if target == "light" { "dark" } else { "light" };
            let original = crate::source::parse_local_theme_source(json!({
                "version": 3,
                "meta": {"id": "my.font", "name": "Font"},
                "common": {"font": {"family": "Base", "weight": "bold", "extension": 1}},
                "pc": {"font": {"size": 24}},
                target: {"common": {"font": {"family": "Old", "extension": 2}}},
                other: {"common": {"font": {"family": "Other", "weight": "black"}}}
            }))
            .unwrap();
            let edited = edit_theme_source_field(
                &original,
                target,
                "common.font",
                Some(json!({"family": "New", "weight": "regular"})),
            )
            .unwrap();
            assert_eq!(edited.value[target]["common"]["font"]["family"], "New");
            assert_eq!(edited.value[target]["common"]["font"]["weight"], "regular");
            assert_eq!(edited.value[target]["common"]["font"]["extension"], 2);
            assert_eq!(edited.value["common"], original.value["common"]);
            assert_eq!(edited.value["pc"], original.value["pc"]);
            assert_eq!(edited.value[other], original.value[other]);

            for value in [
                None,
                Some(json!(null)),
                Some(json!({"family": "New"})),
                Some(json!({"family": "New", "weight": "regular", "size": 24})),
                Some(json!({"family": 42, "weight": "regular"})),
                Some(json!({"family": "New", "weight": "unsupported"})),
            ] {
                let before = original.value.clone();
                assert!(edit_theme_source_field(&original, target, "common.font", value).is_err());
                assert_eq!(original.value, before);
            }
        }
    }

    #[test]
    fn linked_padding_reuses_leaf_edits_and_keeps_other_settings() {
        let source = crate::source::parse_local_theme_source(json!({
            "version": 3,
            "meta": {"id": "my.padding", "name": "Padding"},
            "pc": {"panel": {"paddingY": 9, "paddingTop": 3}},
            "light": {"pc": {"panel": {"paddingLeft": 15}}},
            "dark": {"pc": {"panel": {"paddingTop": 6, "paddingLeft": 17, "gap": 4}}}
        }))
        .unwrap();
        let linked = edit_theme_source_field(
            &source,
            "dark",
            "pc.panel.padding",
            Some(json!({"paddingTop": 12, "paddingBottom": 12})),
        )
        .unwrap();
        assert_eq!(linked.value["dark"]["pc"]["panel"]["paddingTop"], 12.0);
        assert_eq!(linked.value["dark"]["pc"]["panel"]["paddingBottom"], 12.0);
        assert_eq!(linked.value["dark"]["pc"]["panel"]["paddingLeft"], 17.0);
        assert_eq!(linked.value["dark"]["pc"]["panel"]["gap"], 4.0);
        assert_eq!(linked.value["light"], source.value["light"]);
        assert_eq!(linked.value["pc"], source.value["pc"]);

        let reset = edit_theme_source_field(
            &linked,
            "dark",
            "pc.panel.padding",
            Some(json!({"paddingTop": null, "paddingBottom": null})),
        )
        .unwrap();
        assert_eq!(reset.value["pc"]["panel"], json!({"paddingY": 9.0}));
        assert_eq!(
            reset.value["dark"]["pc"]["panel"],
            json!({"paddingLeft": 17.0, "gap": 4.0})
        );
        for invalid in [
            json!({}),
            json!({"paddingTop": 12, "paddingX": 10}),
            json!({"paddingTop": "invalid", "paddingBottom": 12}),
        ] {
            assert!(
                edit_theme_source_field(&source, "dark", "pc.panel.padding", Some(invalid))
                    .is_err()
            );
        }
    }
}
