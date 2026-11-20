//! Resolved mobile appearance. Layout and geometry remain in `keyboard.yaml`.
//!
//! Colors use the same RGBA objects as the desktop resolved JSON. Adapters can
//! render this object directly and use [`resolve_mobile_key_style`] for the
//! current key, mode, and state; no color derivation belongs in adapters.

use std::sync::OnceLock;

use serde_json::{json, Map, Value};

use crate::{
    mix_color, parse_color, with_alpha, FontWeight, ResolvedImeTheme, RgbaColor, DEFAULT_THEME_YAML,
};

/// Resolve validated, variant-merged source `mobile` overrides into appearance.
///
/// `is_dark` is the effective source appearance, not a background-luminance
/// guess. `keyStates.selected.softBackground` is a resolved-only color for
/// function/accent keys; an explicit selected background overrides both forms.
pub fn resolve_mobile_theme(pc: &ResolvedImeTheme, overrides: &Value, is_dark: bool) -> Value {
    resolve_mobile_theme_for_platform(pc, overrides, is_dark, cfg!(target_os = "ios"))
}

fn resolve_mobile_theme_for_platform(
    pc: &ResolvedImeTheme,
    overrides: &Value,
    is_dark: bool,
    ios: bool,
) -> Value {
    static DEFAULTS: OnceLock<Value> = OnceLock::new();
    let defaults = DEFAULTS.get_or_init(|| {
        serde_yaml::from_str(DEFAULT_THEME_YAML).expect("the bundled theme must be valid YAML")
    });
    let mut mobile = defaults["mobile"].clone();
    if is_dark {
        merge(&mut mobile, &defaults["dark"]["mobile"]);
    }
    normalize_colors(&mut mobile);

    let accent = pc
        .ui
        .accent_color
        .unwrap_or(pc.candidate.selected_label_color);
    let keyboard_background = with_alpha(
        mix_color(pc.panel.background, accent, 0.07),
        pc.panel.background.alpha,
    );
    let soft_background = if ios {
        // UIKit kept both blends unrounded until the final resolved color.
        with_alpha(
            mix_color(pc.panel.background, accent, 0.07 + 0.93 * 0.22),
            (255.0_f32
                * if is_dark {
                    0.66 + 0.36 * 0.22
                } else {
                    0.44 + 0.72 * 0.22
                })
            .round() as u8,
        )
    } else {
        with_alpha(
            mix_color(keyboard_background, accent, 0.22),
            pc.panel.background.alpha,
        )
    };
    let mut soft_key = json!({
        "background": color(soft_background),
        "pressedBackground": color(with_alpha(
            mix_color(soft_background, pc.candidate.pressed_background, 0.72),
            soft_background.alpha,
        )),
        "borderColor": color(with_alpha(accent, if is_dark { 107 } else { 153 })),
        "borderWidth": 1,
    });
    for field in ["pressedBackground", "borderColor", "borderWidth"] {
        if overrides["keyboard"]["key"].get(field).is_some() {
            soft_key.as_object_mut().unwrap().remove(field);
        }
    }

    mobile["isDark"] = json!(is_dark);
    mobile["keyboard"]["background"] = color(keyboard_background);
    if ios {
        mobile["keyboard"]["key"]["background"] =
            color(parse_color(if is_dark { "#1C222A61" } else { "#FFFFFF94" }).unwrap());
        if is_dark {
            mobile["keyboard"]["key"]["borderColor"] = color(parse_color("#FFFFFF17").unwrap());
        }
    }
    if pc.candidate.background.alpha > 0 {
        mobile["keyboard"]["key"]["background"] = color(if ios {
            with_alpha(pc.candidate.background, if is_dark { 87 } else { 133 })
        } else {
            pc.candidate.background
        });
    }
    for (field, value) in [
        ("foreground", color(pc.candidate.foreground)),
        ("hintForeground", color(pc.candidate.comment_color)),
        ("pressedBackground", color(pc.candidate.pressed_background)),
        ("pressedForeground", color(pc.candidate.pressed_foreground)),
        (
            "cornerRadius",
            json!(pc.candidate.corner_radius * 10.0 / 11.0),
        ),
        (
            "fontWeight",
            json!(match pc.font.weight {
                FontWeight::SemiBold => FontWeight::Regular,
                weight => weight,
            }),
        ),
    ] {
        mobile["keyboard"]["key"][field] = value;
    }
    mobile["keyboard"]["keyTypes"] = json!({
        "character": {}, "function": soft_key,
        "space": {}, "enter": {}, "backspace": {}, "shift": {}
    });
    mobile["keyboard"]["keyClasses"] = json!({
        "accent": soft_key,
        "accentPunctuation": soft_key,
    });
    if overrides.pointer("/keyboard/key/foreground").is_none() {
        mobile["keyboard"]["keyClasses"]["accent"]["foreground"] =
            color(pc.candidate.selected_foreground);
    }
    mobile["keyboard"]["keyStates"] = json!({
        "selected": {"background": color(pc.candidate.selected_background),
            "softBackground": color(pc.candidate.selected_background),
            "pressedBackground": color(with_alpha(
                mix_color(pc.candidate.selected_background, pc.candidate.pressed_background, 0.52),
                pc.candidate.selected_background.alpha,
            )),
            "foreground": color(pc.candidate.selected_foreground)}
    });
    if overrides
        .pointer("/keyboard/key/pressedBackground")
        .is_some()
    {
        mobile["keyboard"]["keyStates"]["selected"]
            .as_object_mut()
            .unwrap()
            .remove("pressedBackground");
    }
    mobile["keyboard"]["toolbar"]["foreground"] = color(pc.candidate.foreground);
    mobile["keyboard"]["toolbar"]["selectedForeground"] = color(accent);
    for (field, value) in [
        ("foreground", pc.candidate.foreground),
        ("selectedForeground", pc.candidate.selected_foreground),
        ("selectedBackground", pc.candidate.selected_background),
        ("commentColor", pc.candidate.comment_color),
        (
            "preeditForeground",
            if ios {
                pc.candidate.label_color
            } else {
                pc.candidate.foreground
            },
        ),
    ] {
        mobile["candidateBar"][field] = color(value);
    }

    let mut overrides = overrides.clone();
    normalize_colors(&mut overrides);
    merge(&mut mobile, &overrides);
    for (target, inherited) in [
        ("/keyboard/pageBackground", "/keyboard/background"),
        (
            "/keyboard/key/swipeHintForeground",
            "/keyboard/key/hintForeground",
        ),
        (
            "/keyboard/floating/borderColor",
            "/keyboard/key/borderColor",
        ),
        ("/candidateBar/expandedBackground", "/keyboard/background"),
    ] {
        if overrides.pointer(target).is_none() {
            let inherited = mobile.pointer(inherited).unwrap().clone();
            let (parent, field) = target.rsplit_once('/').unwrap();
            mobile.pointer_mut(parent).unwrap()[field] = inherited;
        }
    }
    if let Some(background) = overrides.pointer("/keyboard/keyStates/selected/background") {
        mobile["keyboard"]["keyStates"]["selected"]["softBackground"] = background.clone();
    }
    let mut shift_on = Map::new();
    overlay_colors(&mut shift_on, &mobile["keyboard"]["keyClasses"]["accent"]);
    shift_on.insert(
        "background".into(),
        mobile["keyboard"]["key"]["background"].clone(),
    );
    shift_on.insert("borderColor".into(), color(pc.candidate.selected_border_color));
    overlay_colors(
        &mut shift_on,
        &overrides["keyboard"]["keyStates"]["shiftOn"],
    );
    mobile["keyboard"]["keyStates"]["shiftOn"] = Value::Object(shift_on);
    sanitize_numbers(&mut mobile);
    mobile
}

/// Resolve `key -> keyTypes -> keyClasses -> keyStates` for the current mode.
///
/// Function subtypes inherit `function`; shift inherits `character`. If both
/// built-in classes match, the more specific `accentPunctuation` applies last.
/// States only override colors, and `shiftOn` only applies to a shift action.
pub fn resolve_mobile_key_style(
    mobile: &Value,
    key: &crate::mobile_layout::MobileKey,
    ascii_mode: bool,
    selected: bool,
    shift_on: bool,
) -> Value {
    let keyboard = &mobile["keyboard"];
    let command = effective_command_type(key, ascii_mode);
    let function = matches!(
        command,
        "mode" | "keyboardMode" | "space" | "enter" | "backspace"
    );
    let mut style = keyboard["key"].clone();
    merge(
        &mut style,
        &keyboard["keyTypes"][if function { "function" } else { "character" }],
    );
    if matches!(command, "space" | "enter" | "backspace" | "shift") {
        merge(&mut style, &keyboard["keyTypes"][command]);
    }
    let accent = key.style.as_deref() == Some("accent");
    let label = if ascii_mode {
        key.ascii_label
            .as_deref()
            .or(key.ascii_value.as_deref())
            .unwrap_or(&key.label)
    } else {
        &key.label
    };
    let value = if ascii_mode {
        key.ascii_value.as_deref()
    } else {
        None
    }
    .or(key.value.as_deref())
    .unwrap_or(&key.label);
    let punctuation = ["，", "。", ",", "."]
        .iter()
        .any(|item| *item == label || *item == value);
    if accent {
        merge(&mut style, &keyboard["keyClasses"]["accent"]);
    }
    if punctuation {
        merge(&mut style, &keyboard["keyClasses"]["accentPunctuation"]);
    }
    if selected {
        let mut state = Map::new();
        overlay_colors(&mut state, &keyboard["keyStates"]["selected"]);
        if function || accent || punctuation {
            if let Some(background) = keyboard["keyStates"]["selected"].get("softBackground") {
                state.insert("background".into(), background.clone());
            }
        }
        merge(&mut style, &Value::Object(state));
    }
    if shift_on && command == "shift" {
        let mut state = Map::new();
        overlay_colors(&mut state, &keyboard["keyStates"]["shiftOn"]);
        merge(&mut style, &Value::Object(state));
    }
    style
}

/// Shared source-validation and final-resolution ranges for mobile appearance.
pub(crate) fn sanitize_mobile_number(field: &str, value: f64) -> Option<f64> {
    let (min, max) = match field {
        "borderWidth" => (0.0, 2.0),
        "cornerRadius" => (0.0, 24.0),
        "shadowOffsetY" => (0.0, 4.0),
        "fontScale" | "hintFontScale" => (0.8, 1.3),
        _ => return None,
    };
    value.is_finite().then(|| value.clamp(min, max))
}

fn effective_command_type(key: &crate::mobile_layout::MobileKey, ascii_mode: bool) -> &str {
    if ascii_mode {
        if let Some(action) = &key.ascii_action {
            return &action.command_type;
        }
        if key.ascii_value.is_some() {
            return "input";
        }
    } else if key.rime_value.is_some()
        || key
            .ascii_value
            .as_deref()
            .is_some_and(|value| value != key.value.as_deref().unwrap_or(&key.label))
    {
        return "rimeInput";
    }
    key.action
        .as_ref()
        .map(|action| action.command_type.as_str())
        .unwrap_or("input")
}

fn color(value: RgbaColor) -> Value {
    json!(value)
}

fn is_color_field(field: &str) -> bool {
    matches!(
        field,
        "background"
            | "pageBackground"
            | "foreground"
            | "hintForeground"
            | "pressedBackground"
            | "pressedForeground"
            | "borderColor"
            | "shadowColor"
            | "swipeHintForeground"
            | "selectedForeground"
            | "selectedBackground"
            | "commentColor"
            | "preeditForeground"
            | "preeditBackground"
            | "expandedBackground"
    )
}

fn overlay_colors(target: &mut Map<String, Value>, source: &Value) {
    if let Some(source) = source.as_object() {
        for (field, value) in source {
            if is_color_field(field) {
                target.insert(field.clone(), value.clone());
            }
        }
    }
}

fn merge(target: &mut Value, source: &Value) {
    if let Some(source) = source.as_object() {
        if !target.is_object() {
            *target = json!({});
        }
        for (field, value) in source {
            if value.is_object() {
                merge(&mut target[field], value);
            } else {
                target[field] = value.clone();
            }
        }
    }
}

fn normalize_colors(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        for (field, value) in object {
            if is_color_field(field) {
                if let Some(parsed) = value.as_str().and_then(parse_color) {
                    *value = color(parsed);
                }
            } else {
                normalize_colors(value);
            }
        }
    }
}

fn sanitize_numbers(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        for (field, value) in object {
            if let Some(number) = value
                .as_f64()
                .and_then(|number| sanitize_mobile_number(field, number))
            {
                *value = json!(number);
            } else {
                sanitize_numbers(value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{resolve_theme_with_pending_settings, EffectiveColorScheme};

    fn theme(dark: bool, overrides: Value) -> Value {
        let scheme = if dark {
            EffectiveColorScheme::Dark
        } else {
            EffectiveColorScheme::Light
        };
        let pc = resolve_theme_with_pending_settings(None, None, scheme, None);
        resolve_mobile_theme(&pc, &overrides, dark)
    }

    fn key(value: Value) -> crate::mobile_layout::MobileKey {
        serde_json::from_value(value).unwrap()
    }

    fn style(mobile: &Value, command: &str) -> Value {
        resolve_mobile_key_style(
            mobile,
            &key(json!({"label":"a", "action":{"type":command}})),
            false,
            false,
            false,
        )
    }

    #[test]
    fn defaults_and_derivations_follow_pc_and_yaml() {
        for dark in [false, true] {
            let scheme = if dark {
                EffectiveColorScheme::Dark
            } else {
                EffectiveColorScheme::Light
            };
            let mut pc = resolve_theme_with_pending_settings(None, None, scheme, None);
            pc.panel.background = parse_color("#10203040").unwrap();
            pc.ui.accent_color = parse_color("#A0B0C080");
            let mobile = resolve_mobile_theme_for_platform(&pc, &json!({}), dark, false);
            assert_eq!(mobile["isDark"], dark);
            assert_eq!(
                mobile["keyboard"]["background"],
                color(parse_color("#1A2A3A40").unwrap())
            );
            assert_eq!(
                mobile["keyboard"]["keyTypes"]["function"]["background"],
                color(parse_color("#37475740").unwrap())
            );
            assert_eq!(
                mobile["keyboard"]["keyStates"]["selected"]["softBackground"],
                color(pc.candidate.selected_background)
            );
            assert_eq!(mobile["keyboard"]["key"]["cornerRadius"], 10.0);
            assert_eq!(
                mobile["keyboard"]["key"]["hintForeground"],
                color(pc.candidate.comment_color)
            );
            assert_eq!(
                mobile["candidateBar"]["foreground"],
                color(pc.candidate.foreground)
            );
            assert_eq!(
                mobile["keyboard"]["key"]["background"]["alpha"],
                if dark { 170 } else { 210 }
            );
            pc.candidate.background = parse_color("#12345601").unwrap();
            assert_eq!(
                resolve_mobile_theme_for_platform(&pc, &json!({}), dark, false)["keyboard"]["key"]
                    ["background"],
                color(pc.candidate.background)
            );
            pc.candidate.selected_background.alpha = 128;
            assert_eq!(
                resolve_mobile_theme_for_platform(&pc, &json!({}), dark, false)["keyboard"]
                    ["keyStates"]["selected"]["background"]["alpha"],
                128
            );
            assert_eq!(
                resolve_mobile_theme_for_platform(&pc, &json!({}), dark, false)["keyboard"]
                    ["keyStates"]["selected"]["pressedBackground"],
                color(with_alpha(
                    mix_color(
                        pc.candidate.selected_background,
                        pc.candidate.pressed_background,
                        0.52
                    ),
                    128,
                ))
            );

            let ios = resolve_mobile_theme_for_platform(&pc, &json!({}), dark, true);
            assert_eq!(
                ios["keyboard"]["keyTypes"]["function"]["background"],
                color(parse_color(if dark { "#384858BC" } else { "#38485899" }).unwrap())
            );
            assert_eq!(
                ios["keyboard"]["key"]["background"],
                color(with_alpha(
                    pc.candidate.background,
                    if dark { 87 } else { 133 }
                ))
            );
        }
    }

    #[test]
    fn shift_on_uses_key_background_and_selected_border_unless_overridden() {
        for (scheme, dark) in [
            (EffectiveColorScheme::Light, false),
            (EffectiveColorScheme::Dark, true),
        ] {
            let pc = resolve_theme_with_pending_settings(None, None, scheme, None);
            for ios in [false, true] {
                for overrides in [
                    json!({}),
                    json!({"keyboard":{"key":{"background":"#10203040"},
                        "keyClasses":{"accent":{"background":"#50607080","borderColor":"#90A0B0C0"}}}}),
                    json!({"keyboard":{"keyStates":{"shiftOn":{
                        "background":"#11223344","borderColor":"#55667788"}}}}),
                ] {
                    let mobile = resolve_mobile_theme_for_platform(&pc, &overrides, dark, ios);
                    let shifted = resolve_mobile_key_style(
                        &mobile,
                        &key(json!({"label":"shift","action":{"type":"shift"}})),
                        false,
                        false,
                        true,
                    );
                    let explicit = &overrides["keyboard"]["keyStates"]["shiftOn"];
                    let background = explicit["background"]
                        .as_str()
                        .map(|value| color(parse_color(value).unwrap()))
                        .unwrap_or_else(|| mobile["keyboard"]["key"]["background"].clone());
                    let border = explicit["borderColor"]
                        .as_str()
                        .map(|value| color(parse_color(value).unwrap()))
                        .unwrap_or_else(|| color(pc.candidate.selected_border_color));
                    assert_eq!(shifted["background"], background, "{scheme:?}, ios={ios}");
                    assert_eq!(shifted["borderColor"], border, "{scheme:?}, ios={ios}");
                }
            }
        }
    }

    #[test]
    fn ios_defaults_match_resolved_theme_contract() {
        for (scheme, dark, fixture) in [
            (
                EffectiveColorScheme::Light,
                false,
                include_str!("../tests/fixtures/default-ios-light.json"),
            ),
            (
                EffectiveColorScheme::Dark,
                true,
                include_str!("../tests/fixtures/default-ios-dark.json"),
            ),
        ] {
            let pc = resolve_theme_with_pending_settings(None, None, scheme, None);
            let expected: Value = serde_json::from_str(fixture).unwrap();
            assert_eq!(
                resolve_mobile_theme_for_platform(&pc, &json!({}), dark, true),
                expected["mobile"]
            );
        }
    }

    #[test]
    fn explicit_colors_keep_alpha_and_dependent_defaults() {
        let mobile = theme(
            false,
            json!({"keyboard": {
                "background":"#01020304", "key":{"background":"#10203000", "hintForeground":"#40506070", "borderColor":"#708090A0", "borderWidth":0.5, "pressedBackground":"#22334455", "foreground":"#66778899", "fontWeight":"black"},
                "keyStates":{"selected":{"background":"#AABBCC09"}},
                "keyClasses":{"accent":{"background":"#11223344", "cornerRadius":3}}
            }}),
        );
        assert_eq!(mobile["keyboard"]["key"]["fontWeight"], "black");
        for path in [
            "/keyboard/background",
            "/keyboard/pageBackground",
            "/candidateBar/expandedBackground",
        ] {
            assert_eq!(
                mobile.pointer(path).unwrap(),
                &color(parse_color("#01020304").unwrap())
            );
        }
        assert_eq!(mobile["keyboard"]["key"]["background"]["alpha"], 0);
        assert_eq!(
            mobile["keyboard"]["key"]["swipeHintForeground"]["alpha"],
            112
        );
        assert_eq!(mobile["keyboard"]["floating"]["borderColor"]["alpha"], 160);
        assert_eq!(
            mobile["keyboard"]["keyStates"]["shiftOn"]["background"]["alpha"],
            0
        );
        assert!(mobile["keyboard"]["keyStates"]["shiftOn"]
            .get("cornerRadius")
            .is_none());
        for spec in [
            json!({"label":"a"}),
            json!({"label":"a","style":"accent"}),
            json!({"label":"."}),
            json!({"label":"space","action":{"type":"space"}}),
        ] {
            let style = resolve_mobile_key_style(&mobile, &key(spec), false, false, false);
            for field in [
                "pressedBackground",
                "borderColor",
                "borderWidth",
                "foreground",
            ] {
                assert_eq!(style[field], mobile["keyboard"]["key"][field]);
            }
        }
        for command in ["input", "space"] {
            let selected = resolve_mobile_key_style(
                &mobile,
                &key(json!({"label":"a","action":{"type":command}})),
                false,
                true,
                false,
            );
            assert_eq!(selected["background"]["alpha"], 9);
            assert_eq!(
                selected["pressedBackground"],
                mobile["keyboard"]["key"]["pressedBackground"]
            );
        }
        let mobile = theme(
            false,
            json!({"keyboard": {
                "key":{"pressedBackground":"#112233", "borderColor":"#445566", "borderWidth":0.5, "foreground":"#778899"},
                "keyTypes":{"function":{"pressedBackground":"#223344", "borderColor":"#556677", "borderWidth":1.5, "foreground":"#8899AA"}},
                "keyClasses":{"accent":{"pressedBackground":"#334455", "borderColor":"#667788", "borderWidth":2, "foreground":"#99AABB"}},
                "keyStates":{"selected":{"pressedBackground":"#445566"}}
            }}),
        );
        for (spec, expected) in [
            (
                json!({"label":"space","action":{"type":"space"}}),
                &mobile["keyboard"]["keyTypes"]["function"],
            ),
            (
                json!({"label":"space","action":{"type":"space"},"style":"accent"}),
                &mobile["keyboard"]["keyClasses"]["accent"],
            ),
        ] {
            let style = resolve_mobile_key_style(&mobile, &key(spec), false, false, false);
            for field in [
                "pressedBackground",
                "borderColor",
                "borderWidth",
                "foreground",
            ] {
                assert_eq!(style[field], expected[field]);
            }
        }
        let selected =
            resolve_mobile_key_style(&mobile, &key(json!({"label":"a"})), false, true, false);
        assert_eq!(
            selected["pressedBackground"],
            color(parse_color("#445566").unwrap())
        );
    }

    #[test]
    fn effective_action_sets_function_and_character_types() {
        let mobile = theme(
            false,
            json!({"keyboard":{"keyTypes":{
                "character":{"foreground":"#111111"}, "function":{"foreground":"#222222"},
                "space":{"hintForeground":"#333333"}, "shift":{"borderColor":"#444444"}
            }}}),
        );
        for command in ["mode", "keyboardMode", "space", "enter", "backspace"] {
            assert_eq!(
                style(&mobile, command)["foreground"],
                color(parse_color("#222222").unwrap())
            );
        }
        for command in [
            "input",
            "directInput",
            "openPage",
            "floating",
            "reset",
            "nextInputMethod",
            "unknown",
            "shift",
        ] {
            assert_eq!(
                style(&mobile, command)["foreground"],
                color(parse_color("#111111").unwrap())
            );
        }
        assert_eq!(
            style(&mobile, "space")["hintForeground"],
            color(parse_color("#333333").unwrap())
        );
        assert_eq!(
            style(&mobile, "shift")["borderColor"],
            color(parse_color("#444444").unwrap())
        );
        let key =
            key(json!({"label":"a","action":{"type":"space"},"asciiAction":{"type":"unknown"}}));
        assert_eq!(
            resolve_mobile_key_style(&mobile, &key, false, false, false)["foreground"],
            color(parse_color("#222222").unwrap())
        );
        assert_eq!(
            resolve_mobile_key_style(&mobile, &key, true, false, false)["foreground"],
            color(parse_color("#111111").unwrap())
        );
        for value in [
            json!({"label":"a"}),
            json!({"label":"a", "action":{"type":"space"}, "asciiValue":"b"}),
            json!({"label":"a", "action":{"type":"space"}, "rimeValue":"b"}),
        ] {
            let key = self::key(value);
            for ascii_mode in [false, true] {
                if ascii_mode && key.rime_value.is_some() {
                    continue;
                }
                assert_eq!(
                    resolve_mobile_key_style(&mobile, &key, ascii_mode, false, false)["foreground"],
                    color(parse_color("#111111").unwrap())
                );
            }
        }
    }

    #[test]
    fn selectors_layer_and_only_current_mode_punctuation_matches() {
        let mobile = theme(
            false,
            json!({"keyboard":{
                "key":{"background":"#111111", "cornerRadius":5},
                "keyTypes":{"character":{"background":"#222222"}},
                "keyClasses":{"accent":{"background":"#333333"},"accentPunctuation":{"background":"#444444"}},
                "keyStates":{"selected":{"background":"#555555","cornerRadius":20},"shiftOn":{"background":"#666666","cornerRadius":21}}
            }}),
        );
        let key_spec =
            key(json!({"label":"，", "asciiLabel":"a", "asciiValue":"a", "style":"accent"}));
        assert_eq!(
            resolve_mobile_key_style(&mobile, &key_spec, false, false, false)["background"],
            color(parse_color("#444444").unwrap())
        );
        assert_eq!(
            resolve_mobile_key_style(&mobile, &key_spec, true, false, false)["background"],
            color(parse_color("#333333").unwrap())
        );
        let selected = resolve_mobile_key_style(&mobile, &key_spec, true, true, true);
        assert_eq!(
            selected["background"],
            color(parse_color("#555555").unwrap())
        );
        assert_eq!(selected["cornerRadius"], 5.0);
        let shift = key(json!({"label":"shift", "action":{"type":"shift"}}));
        let shifted = resolve_mobile_key_style(&mobile, &shift, false, true, true);
        assert_eq!(
            shifted["background"],
            color(parse_color("#666666").unwrap())
        );
        assert_eq!(shifted["cornerRadius"], 5.0);
        for field in ["label", "value"] {
            for punctuation in ["，", "。", ",", "."] {
                let mut key = json!({"label":"a"});
                key[field] = json!(punctuation);
                assert_eq!(
                    resolve_mobile_key_style(&mobile, &super::tests::key(key), false, false, false)
                        ["background"],
                    color(parse_color("#444444").unwrap())
                );
            }
        }
    }

    #[test]
    fn selected_defaults_are_resolved_for_each_key_class_and_appearance() {
        for dark in [false, true] {
            let mobile = theme(dark, json!({}));
            for (key, soft) in [
                (json!({"label":"a"}), false),
                (json!({"label":"a","style":"accent"}), true),
                (json!({"label":"."}), true),
                (json!({"label":"space","action":{"type":"space"}}), true),
            ] {
                let selected =
                    resolve_mobile_key_style(&mobile, &self::key(key), false, true, false);
                assert_eq!(
                    selected["background"],
                    mobile["keyboard"]["keyStates"]["selected"]
                        [if soft { "softBackground" } else { "background" }]
                );
                if !soft {
                    assert_eq!(selected["background"]["alpha"], 255);
                }
            }
        }
    }

    #[test]
    fn numeric_limits_share_the_validation_ranges() {
        for (field, low, high) in [
            ("borderWidth", 0.0, 2.0),
            ("cornerRadius", 0.0, 24.0),
            ("shadowOffsetY", 0.0, 4.0),
            ("fontScale", 0.8, 1.3),
            ("hintFontScale", 0.8, 1.3),
        ] {
            assert_eq!(sanitize_mobile_number(field, -1.0), Some(low));
            assert_eq!(sanitize_mobile_number(field, 100.0), Some(high));
            assert_eq!(sanitize_mobile_number(field, f64::NAN), None);
        }
        let mobile = theme(
            false,
            json!({"keyboard":{"key":{"cornerRadius":100,"fontScale":0}}}),
        );
        assert_eq!(mobile["keyboard"]["key"]["cornerRadius"], 24.0);
        assert_eq!(mobile["keyboard"]["key"]["fontScale"], 0.8);
    }
}
