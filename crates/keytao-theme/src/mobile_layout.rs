//! Mobile soft keyboard layout.
//!
//! The soft keyboard is an Android/iOS adapter concern: rows, layers, swipe and
//! long press commands only mean something to a platform that draws its own
//! keys. It therefore lives in its own document (`keyboard.yaml`) and its own
//! resolved model, and never becomes part of the cross-platform
//! [`ResolvedImeTheme`](crate::ResolvedImeTheme), which stays limited to
//! candidate panel and mode hint semantics.
//!
//! For backwards compatibility a legacy `theme.yaml` carrying a `keyboard:`
//! section is still accepted as a layout source: pass its path to
//! [`resolve_mobile_layout_from_paths`].

use serde::{Deserialize, Deserializer, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

use crate::{assign, clamp, PartialTheme};

pub const DEFAULT_MOBILE_LAYOUT_YAML: &str = include_str!("../default-keyboard.yaml");

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileLayout {
    pub height: f32,
    pub candidate_bar_height: f32,
    /// Fixed cross-platform row height for clipboard-history entries.
    pub clipboard_row_height: f32,
    /// Fixed cross-platform hit width for each clipboard delete affordance.
    pub clipboard_delete_hit_width: f32,
    pub bottom_inset: f32,
    pub horizontal_gap: f32,
    pub vertical_gap: f32,
    pub outer_inset: f32,
    pub max_key_height: f32,
    pub floating: MobileFloatingLayout,
    pub rows: Vec<Vec<MobileKey>>,
    pub number_rows: Vec<Vec<MobileKey>>,
    pub symbol_rows: Vec<Vec<MobileKey>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub layers: BTreeMap<String, Vec<Vec<MobileKey>>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileFloatingLayout {
    pub margin: f32,
    pub portrait: MobileFloatingProfile,
    pub landscape: MobileFloatingProfile,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileFloatingProfile {
    pub enabled: bool,
    pub scale: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileKey {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ascii_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ascii_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rime_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<MobileCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ascii_action: Option<MobileCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swipe_up: Option<MobileCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swipe_down: Option<MobileCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_press: Option<MobileCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ascii_long_press: Option<MobileCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_span: Option<u8>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stack: Vec<MobileKeyStackItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileKeyStackItem {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ascii_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ascii_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rime_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<MobileCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ascii_action: Option<MobileCommand>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileCommand {
    #[serde(rename = "type")]
    pub command_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_value: Option<String>,
}

impl Default for MobileLayout {
    fn default() -> Self {
        Self {
            height: 266.0,
            candidate_bar_height: 52.0,
            clipboard_row_height: 44.0,
            clipboard_delete_hit_width: 44.0,
            bottom_inset: 0.0,
            horizontal_gap: 4.0,
            vertical_gap: 5.0,
            outer_inset: 5.0,
            max_key_height: 54.0,
            floating: MobileFloatingLayout::default(),
            rows: Vec::new(),
            number_rows: Vec::new(),
            symbol_rows: Vec::new(),
            layers: BTreeMap::new(),
        }
    }
}

impl Default for MobileFloatingLayout {
    fn default() -> Self {
        Self {
            margin: 8.0,
            portrait: MobileFloatingProfile {
                enabled: false,
                scale: 0.88,
            },
            landscape: MobileFloatingProfile {
                enabled: true,
                scale: 0.62,
            },
        }
    }
}

pub fn default_mobile_layout_yaml() -> &'static str {
    DEFAULT_MOBILE_LAYOUT_YAML
}

/// Resolves the mobile keyboard layout from the builtin default plus up to two
/// overlays. Each path may be a `keyboard.yaml` document or a legacy
/// `theme.yaml` that still carries a `keyboard:` section.
pub fn resolve_mobile_layout_from_paths(
    default_layout_path: Option<&Path>,
    user_layout_path: Option<&Path>,
) -> MobileLayout {
    let mut layout = MobileLayout::default();
    for partial in layout_partials(default_layout_path, user_layout_path) {
        layout.apply(partial);
    }
    sanitize(layout)
}

pub fn mobile_layout_json(layout: &MobileLayout) -> Result<String, serde_json::Error> {
    serde_json::to_string(layout)
}

fn layout_partials(
    default_layout_path: Option<&Path>,
    user_layout_path: Option<&Path>,
) -> Vec<PartialMobileLayout> {
    let mut partials = Vec::new();
    if let Some(partial) = parse_mobile_layout_yaml(DEFAULT_MOBILE_LAYOUT_YAML) {
        partials.push(partial);
    }
    for path in [default_layout_path, user_layout_path]
        .into_iter()
        .flatten()
    {
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        if let Some(partial) = parse_mobile_layout_yaml(&content) {
            partials.push(partial);
        }
    }
    partials
}

fn parse_mobile_layout_yaml(content: &str) -> Option<PartialMobileLayout> {
    if let Ok(file) = serde_yaml::from_str::<MobileLayoutFile>(content) {
        return file.into_layout();
    }
    serde_yaml::from_str::<PartialTheme>(content)
        .ok()
        .and_then(|theme| theme.keyboard)
}

fn sanitize(mut layout: MobileLayout) -> MobileLayout {
    layout.height = clamp(layout.height, 160.0, 420.0);
    layout.candidate_bar_height = clamp(layout.candidate_bar_height, 36.0, 96.0);
    layout.bottom_inset = clamp(layout.bottom_inset, 0.0, 80.0);
    layout.horizontal_gap = clamp(layout.horizontal_gap, 0.0, 24.0);
    layout.vertical_gap = clamp(layout.vertical_gap, 0.0, 24.0);
    layout.outer_inset = clamp(layout.outer_inset, 0.0, 32.0);
    layout.max_key_height = clamp(layout.max_key_height, 36.0, 84.0);
    layout.floating.margin = clamp(layout.floating.margin, 0.0, 24.0);
    layout.floating.portrait.scale = clamp(layout.floating.portrait.scale, 0.70, 1.0);
    layout.floating.landscape.scale = clamp(layout.floating.landscape.scale, 0.45, 1.0);
    layout.layers.retain(|name, rows| {
        !name.trim().is_empty()
            && name != "letters"
            && name != "numbers"
            && name != "symbols"
            && !rows.is_empty()
    });
    layout
}

#[derive(Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MobileLayoutFile {
    keyboard: Option<PartialMobileLayout>,
    #[serde(flatten)]
    root: PartialMobileLayout,
}

impl MobileLayoutFile {
    fn into_layout(self) -> Option<PartialMobileLayout> {
        self.keyboard
            .or_else(|| self.root.has_any_value().then_some(self.root))
    }
}

#[derive(Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PartialMobileLayout {
    version: Option<u32>,
    #[serde(alias = "keyboardHeightDp")]
    height: Option<f32>,
    #[serde(alias = "candidateBarHeightDp")]
    candidate_bar_height: Option<f32>,
    #[serde(alias = "keyboardBottomInsetDp")]
    bottom_inset: Option<f32>,
    #[serde(alias = "horizontalGapDp")]
    horizontal_gap: Option<f32>,
    #[serde(alias = "verticalGapDp")]
    vertical_gap: Option<f32>,
    #[serde(alias = "outerInsetDp")]
    outer_inset: Option<f32>,
    #[serde(alias = "maxKeyHeightDp")]
    max_key_height: Option<f32>,
    floating: Option<PartialMobileFloatingLayout>,
    rows: Option<Vec<Vec<MobileKey>>>,
    number_rows: Option<Vec<Vec<MobileKey>>>,
    symbol_rows: Option<Vec<Vec<MobileKey>>>,
    #[serde(
        default,
        alias = "pages",
        alias = "keyboards",
        deserialize_with = "optional_layers"
    )]
    layers: Option<BTreeMap<String, Vec<Vec<MobileKey>>>>,
}

#[derive(Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialMobileFloatingLayout {
    #[serde(alias = "marginDp")]
    margin: Option<f32>,
    portrait: Option<PartialMobileFloatingProfile>,
    landscape: Option<PartialMobileFloatingProfile>,
}

#[derive(Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialMobileFloatingProfile {
    enabled: Option<bool>,
    scale: Option<f32>,
}

#[derive(Clone, Deserialize)]
#[serde(untagged)]
enum PartialLayerRows {
    Rows(Vec<Vec<MobileKey>>),
    Object { rows: Vec<Vec<MobileKey>> },
}

impl PartialLayerRows {
    fn into_rows(self) -> Vec<Vec<MobileKey>> {
        match self {
            Self::Rows(rows) | Self::Object { rows } => rows,
        }
    }
}

fn optional_layers<'de, D>(
    deserializer: D,
) -> Result<Option<BTreeMap<String, Vec<Vec<MobileKey>>>>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<BTreeMap<String, PartialLayerRows>>::deserialize(deserializer)?;
    Ok(raw.map(|layers| {
        layers
            .into_iter()
            .filter_map(|(name, rows)| {
                let rows = rows.into_rows();
                (!rows.is_empty()).then_some((name, rows))
            })
            .collect()
    }))
}

impl PartialMobileLayout {
    fn has_any_value(&self) -> bool {
        self.version.is_some()
            || self.height.is_some()
            || self.candidate_bar_height.is_some()
            || self.bottom_inset.is_some()
            || self.horizontal_gap.is_some()
            || self.vertical_gap.is_some()
            || self.outer_inset.is_some()
            || self.max_key_height.is_some()
            || self.floating.is_some()
            || self.rows.is_some()
            || self.number_rows.is_some()
            || self.symbol_rows.is_some()
            || self
                .layers
                .as_ref()
                .is_some_and(|layers| !layers.is_empty())
    }
}

impl MobileLayout {
    fn apply(&mut self, partial: PartialMobileLayout) {
        assign(&mut self.height, partial.height);
        assign(&mut self.candidate_bar_height, partial.candidate_bar_height);
        assign(&mut self.bottom_inset, partial.bottom_inset);
        assign(&mut self.horizontal_gap, partial.horizontal_gap);
        assign(&mut self.vertical_gap, partial.vertical_gap);
        assign(&mut self.outer_inset, partial.outer_inset);
        assign(&mut self.max_key_height, partial.max_key_height);
        if let Some(floating) = partial.floating {
            self.floating.apply(floating);
        }
        assign(&mut self.rows, partial.rows);
        assign(&mut self.number_rows, partial.number_rows);
        assign(&mut self.symbol_rows, partial.symbol_rows);
        if let Some(layers) = partial.layers {
            if !layers.is_empty() {
                self.layers.extend(layers);
            }
        }
    }
}

impl MobileFloatingLayout {
    fn apply(&mut self, partial: PartialMobileFloatingLayout) {
        assign(&mut self.margin, partial.margin);
        if let Some(portrait) = partial.portrait {
            self.portrait.apply(portrait);
        }
        if let Some(landscape) = partial.landscape {
            self.landscape.apply(landscape);
        }
    }
}

impl MobileFloatingProfile {
    fn apply(&mut self, partial: PartialMobileFloatingProfile) {
        assign(&mut self.enabled, partial.enabled);
        assign(&mut self.scale, partial.scale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_yaml_resolves() {
        let layout = resolve_mobile_layout_from_paths(None, None);
        assert_eq!(layout.height, 266.0);
        assert_eq!(layout.clipboard_row_height, 44.0);
        assert_eq!(layout.clipboard_delete_hit_width, 44.0);
        assert_eq!(layout.rows.len(), 4);
        assert_eq!(layout.rows[0][0].label, "q");
        assert_eq!(layout.number_rows[0][0].label, "+");
        assert_eq!(layout.symbol_rows[0][0].label, "中文");
        assert_eq!(layout.symbol_rows[1][0].label, "【");
        assert!(!layout.floating.portrait.enabled);
        assert_eq!(layout.floating.portrait.scale, 0.88);
        assert!(layout.floating.landscape.enabled);
        assert_eq!(layout.floating.landscape.scale, 0.62);
        let json = mobile_layout_json(&layout).unwrap();
        assert!(json.contains("\"numberRows\""));
        assert!(json.contains("\"symbols_marks\""));
        assert!(json.contains("\"floating\""));
        assert!(json.contains("\"clipboardRowHeight\":44.0"));
        assert!(json.contains("\"clipboardDeleteHitWidth\":44.0"));
    }

    #[test]
    fn shipped_default_layout_has_valid_layer_references() {
        use std::collections::BTreeSet;

        const KNOWN_EDIT_VERBS: &[&str] = &[
            "clearAll",
            "copy",
            "cursorDown",
            "cursorLeft",
            "cursorRight",
            "cursorUp",
            "cut",
            "forwardDelete",
            "lineEnd",
            "lineStart",
            "paste",
            "pasteText",
            "redo",
            "repeatCommit",
            "selectAll",
            "selectLeft",
            "selectRight",
            "tab",
            "toggleSelection",
            "undo",
        ];
        const KNOWN_PANELS: &[&str] = &["clipboard", "close", "home", "rime", "selection"];

        fn collect_command_references(key: &MobileKey, references: &mut BTreeSet<String>) {
            fn collect(command: Option<&MobileCommand>, references: &mut BTreeSet<String>) {
                let Some(command) = command else {
                    return;
                };
                let value = || {
                    command
                        .value
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or_else(|| {
                            panic!("{} command must name a value", command.command_type)
                        })
                };
                match command.command_type.as_str() {
                    "keyboardMode" => {
                        references.insert(value().to_owned());
                    }
                    "edit" => assert!(
                        KNOWN_EDIT_VERBS.contains(&value()),
                        "unknown edit verb {}",
                        value()
                    ),
                    "panel" => {
                        assert!(KNOWN_PANELS.contains(&value()), "unknown panel {}", value())
                    }
                    _ => {}
                }
            }

            for command in [
                key.action.as_ref(),
                key.ascii_action.as_ref(),
                key.swipe_up.as_ref(),
                key.swipe_down.as_ref(),
                key.long_press.as_ref(),
                key.ascii_long_press.as_ref(),
            ] {
                collect(command, references);
            }
            for item in &key.stack {
                collect(item.action.as_ref(), references);
                collect(item.ascii_action.as_ref(), references);
            }
        }

        let partial = parse_mobile_layout_yaml(DEFAULT_MOBILE_LAYOUT_YAML)
            .expect("shipped default keyboard YAML must parse");
        let rows = partial.rows.as_ref().expect("letters layer must exist");
        let number_rows = partial
            .number_rows
            .as_ref()
            .expect("numbers layer must exist");
        let symbol_rows = partial
            .symbol_rows
            .as_ref()
            .expect("symbols layer must exist");
        let layers = partial.layers.as_ref().expect("custom layers must exist");

        for (name, rows) in [
            ("letters", rows),
            ("numbers", number_rows),
            ("symbols", symbol_rows),
        ] {
            assert!(!rows.is_empty(), "built-in layer {name} must not be empty");
            assert!(
                rows.iter().all(|row| !row.is_empty()),
                "built-in layer {name} must not contain an empty row"
            );
        }

        let reserved = ["letters", "numbers", "symbols"];
        assert!(
            !layers.is_empty(),
            "shipped default must define custom layers"
        );
        for (name, rows) in layers {
            assert!(
                !reserved.contains(&name.as_str()),
                "custom layer {name} uses a reserved name"
            );
            assert!(!rows.is_empty(), "custom layer {name} must not be empty");
            assert!(
                rows.iter().all(|row| !row.is_empty()),
                "custom layer {name} must not contain an empty row"
            );
        }

        let mut references = BTreeSet::new();
        for row in rows.iter().chain(number_rows).chain(symbol_rows) {
            for key in row {
                collect_command_references(key, &mut references);
            }
        }
        for rows in layers.values() {
            for key in rows.iter().flatten() {
                collect_command_references(key, &mut references);
            }
        }

        for reference in references {
            assert!(
                reserved.contains(&reference.as_str()) || layers.contains_key(&reference),
                "keyboardMode references missing layer {reference}"
            );
        }
    }

    #[test]
    fn layout_yaml_merges_and_clamps_floating_profiles() {
        let path = std::env::temp_dir().join(format!(
            "keytao-keyboard-floating-{}-{}.yaml",
            std::process::id(),
            line!()
        ));
        fs::write(
            &path,
            "floating:\n  margin: 40\n  portrait:\n    enabled: true\n    scale: 0.42\n  landscape:\n    enabled: false\n    scale: 0.42\n",
        )
        .unwrap();

        let layout = resolve_mobile_layout_from_paths(None, Some(&path));
        fs::remove_file(path).ok();

        assert_eq!(layout.floating.margin, 24.0);
        assert!(layout.floating.portrait.enabled);
        assert_eq!(layout.floating.portrait.scale, 0.70);
        assert!(!layout.floating.landscape.enabled);
        assert_eq!(layout.floating.landscape.scale, 0.45);
    }

    #[test]
    fn layout_yaml_accepts_custom_layers() {
        let path = std::env::temp_dir().join(format!(
            "keytao-keyboard-{}-{}.yaml",
            std::process::id(),
            line!()
        ));
        fs::write(
            &path,
            "height: 300\nlayers:\n  emoji:\n    - [ { label: \"🙂\", value: \"🙂\" } ]\n",
        )
        .unwrap();

        let layout = resolve_mobile_layout_from_paths(None, Some(&path));
        fs::remove_file(path).ok();

        assert_eq!(layout.height, 300.0);
        assert_eq!(layout.layers["emoji"][0][0].label, "🙂");
    }

    #[test]
    fn layout_yaml_accepts_object_layer_rows() {
        let path = std::env::temp_dir().join(format!(
            "keytao-keyboard-{}-{}.yaml",
            std::process::id(),
            line!()
        ));
        fs::write(
            &path,
            "layers:\n  emoji:\n    rows:\n      - [ { label: \"🙂\", value: \"🙂\" } ]\n",
        )
        .unwrap();

        let layout = resolve_mobile_layout_from_paths(None, Some(&path));
        fs::remove_file(path).ok();

        assert_eq!(layout.layers["emoji"][0][0].label, "🙂");
    }

    #[test]
    fn empty_layers_do_not_clear_default_layers() {
        let path = std::env::temp_dir().join(format!(
            "keytao-keyboard-{}-{}.yaml",
            std::process::id(),
            line!()
        ));
        fs::write(&path, "layers: {}\n").unwrap();

        let layout = resolve_mobile_layout_from_paths(None, Some(&path));
        fs::remove_file(path).ok();

        assert!(layout.layers.contains_key("symbols_en"));
        assert!(layout.layers.contains_key("symbols_math"));
    }

    #[test]
    fn legacy_theme_yaml_keyboard_section_is_still_a_layout_source() {
        let path = std::env::temp_dir().join(format!(
            "keytao-legacy-theme-{}-{}.yaml",
            std::process::id(),
            line!()
        ));
        fs::write(
            &path,
            "version: 2\npanel:\n  orientation: horizontal\nkeyboard:\n  height: 300\n  rows:\n    - [ { label: \"z\", value: \"z\" } ]\n",
        )
        .unwrap();

        let layout = resolve_mobile_layout_from_paths(None, Some(&path));
        fs::remove_file(path).ok();

        assert_eq!(layout.height, 300.0);
        assert_eq!(layout.rows.len(), 1);
        assert_eq!(layout.rows[0][0].label, "z");
    }
}
