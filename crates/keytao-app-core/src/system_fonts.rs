use std::{collections::BTreeMap, sync::OnceLock};

use fontdb::{Database, ID, Language, Source, Style};
use keytao_theme::font_resolution::system_font_database;
use ttf_parser::{Face, RawFace, Tag, name_id};

#[derive(Clone, Debug)]
pub struct SystemFontFamily {
    pub family: String,
    pub display_name: String,
    pub weights: Vec<String>,
}

struct FamilyEntry {
    info: SystemFontFamily,
    preview_face: ID,
    preview_weight: u16,
}

struct FontCatalog {
    database: &'static Database,
    families: BTreeMap<String, FamilyEntry>,
}

static CATALOG: OnceLock<FontCatalog> = OnceLock::new();

const WEIGHTS: [&str; 9] = [
    "ultralight",
    "thin",
    "light",
    "regular",
    "medium",
    "semibold",
    "bold",
    "heavy",
    "black",
];

fn theme_weight(weight: u16) -> &'static str {
    WEIGHTS
        .iter()
        .enumerate()
        .min_by_key(|(index, _)| weight.abs_diff((*index as u16 + 1) * 100))
        .map(|(_, name)| *name)
        .unwrap()
}

fn supports_chinese(face: &Face<'_>) -> bool {
    face.glyph_index('中').is_some() && face.glyph_index('汉').is_some()
}

fn chinese_family_name(face: &Face<'_>) -> Option<String> {
    face.names()
        .into_iter()
        .filter_map(|name| {
            let language_rank = match name.language() {
                Language::Chinese_PeoplesRepublicOfChina => 0,
                Language::Chinese_Singapore => 1,
                Language::Chinese_Taiwan
                | Language::Chinese_HongKongSAR
                | Language::Chinese_MacaoSAR => 2,
                _ => return None,
            };
            let name_rank = match name.name_id {
                name_id::TYPOGRAPHIC_FAMILY => 0,
                name_id::FAMILY => 1,
                _ => return None,
            };
            let value = name.to_string().filter(|value| !value.is_empty())?;
            Some(((language_rank, name_rank), value))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, value)| value)
}

fn catalog() -> &'static FontCatalog {
    CATALOG.get_or_init(|| {
        let database = system_font_database();
        let mut families = BTreeMap::<String, FamilyEntry>::new();
        for info in database.faces().filter(|info| info.style == Style::Normal) {
            let Some((family, _)) = info.families.first() else {
                continue;
            };
            // Hidden system faces (".SF…", ".LastResort") are not user-selectable.
            if family.starts_with('.') || family == "LastResort" {
                continue;
            }
            let Some(display_name) = database
                .with_face_data(info.id, |data, index| {
                    let face = Face::parse(data, index).ok()?;
                    supports_chinese(&face).then(|| chinese_family_name(&face))
                })
                .flatten()
            else {
                continue;
            };
            let weight = info.weight.0;
            let entry = families
                .entry(family.clone())
                .or_insert_with(|| FamilyEntry {
                    info: SystemFontFamily {
                        family: family.clone(),
                        display_name: display_name.clone().unwrap_or_else(|| family.clone()),
                        weights: Vec::new(),
                    },
                    preview_face: info.id,
                    preview_weight: weight,
                });
            if entry.info.display_name == entry.info.family {
                if let Some(display_name) = display_name {
                    entry.info.display_name = display_name;
                }
            }
            let mapped_weight = theme_weight(weight);
            if !entry
                .info
                .weights
                .iter()
                .any(|value| value == mapped_weight)
            {
                entry.info.weights.push(mapped_weight.to_owned());
            }
            if weight.abs_diff(400) < entry.preview_weight.abs_diff(400) {
                entry.preview_face = info.id;
                entry.preview_weight = weight;
            }
        }
        for entry in families.values_mut() {
            entry
                .info
                .weights
                .sort_by_key(|weight| WEIGHTS.iter().position(|value| value == weight).unwrap());
        }
        FontCatalog { database, families }
    })
}

pub fn list_system_font_families() -> Result<Vec<SystemFontFamily>, String> {
    let mut families: Vec<_> = catalog()
        .families
        .values()
        .map(|entry| entry.info.clone())
        .collect();
    families.sort_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then_with(|| left.family.cmp(&right.family))
    });
    Ok(families)
}

pub fn system_font_path(family: &str) -> Option<String> {
    let catalog = catalog();
    let face = catalog
        .database
        .face(catalog.families.get(family)?.preview_face)?;
    match &face.source {
        Source::File(path) | Source::SharedFile(path, _) => path.to_str().map(str::to_owned),
        Source::Binary(_) => None,
    }
}

pub fn load_system_font_bytes(family: String) -> Result<Vec<u8>, String> {
    let catalog = catalog();
    let entry = catalog
        .families
        .get(&family)
        .ok_or_else(|| format!("Unknown system font family: {family}"))?;
    catalog
        .database
        .with_face_data(entry.preview_face, standalone_face)
        .ok_or_else(|| format!("Failed to read system font family: {family}"))?
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

fn standalone_face(data: &[u8], index: u32) -> Result<Vec<u8>, String> {
    if ttf_parser::fonts_in_collection(data).is_none() {
        return Ok(data.to_vec());
    }
    // FontLoader always uses face zero, so extract the selected collection face.
    let raw = RawFace::parse(data, index).map_err(|error| error.to_string())?;
    let count = raw.table_records.len();
    if count == 0 {
        return Err("Font has no tables".to_owned());
    }
    let table_directory_size = count
        .checked_mul(16)
        .ok_or_else(|| "Font has too many tables".to_owned())?;
    let directory_len = 12 + usize::from(table_directory_size);
    let mut font_len = directory_len;
    for record in raw.table_records {
        let table = raw
            .table(record.tag)
            .ok_or_else(|| "Invalid font table offset".to_owned())?;
        font_len = table
            .len()
            .checked_next_multiple_of(4)
            .and_then(|len| font_len.checked_add(len))
            .filter(|len| u32::try_from(*len).is_ok())
            .ok_or_else(|| "Font is too large".to_owned())?;
    }
    let mut font = Vec::new();
    font.try_reserve_exact(font_len)
        .map_err(|error| error.to_string())?;
    font.resize(directory_len, 0);
    let cff = raw.table(Tag::from_bytes(b"CFF ")).is_some()
        || raw.table(Tag::from_bytes(b"CFF2")).is_some();
    font[..4].copy_from_slice(if cff { b"OTTO" } else { b"\0\x01\0\0" });
    let selector = (u16::BITS - 1 - count.leading_zeros()) as u16;
    let search_range = (1u16 << selector) * 16;
    font[4..6].copy_from_slice(&count.to_be_bytes());
    font[6..8].copy_from_slice(&search_range.to_be_bytes());
    font[8..10].copy_from_slice(&selector.to_be_bytes());
    font[10..12].copy_from_slice(&(table_directory_size - search_range).to_be_bytes());
    let mut head_offset = None;
    for (table_index, record) in raw.table_records.into_iter().enumerate() {
        let table = raw
            .table(record.tag)
            .ok_or_else(|| "Invalid font table offset".to_owned())?;
        let offset = font.len();
        font.extend_from_slice(table);
        font.resize((font.len() + 3) & !3, 0);
        if record.tag == Tag::from_bytes(b"head") {
            if table.len() < 12 {
                return Err("Invalid font head table".to_owned());
            }
            font[offset + 8..offset + 12].fill(0);
            head_offset = Some(offset);
        }
        let start = 12 + table_index * 16;
        let table_checksum = checksum(&font[offset..offset + table.len()]);
        font[start..start + 4].copy_from_slice(&record.tag.to_bytes());
        font[start + 4..start + 8].copy_from_slice(&table_checksum.to_be_bytes());
        let offset = u32::try_from(offset).map_err(|error| error.to_string())?;
        font[start + 8..start + 12].copy_from_slice(&offset.to_be_bytes());
        font[start + 12..start + 16].copy_from_slice(&record.length.to_be_bytes());
    }
    if let Some(offset) = head_offset {
        let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&font));
        font[offset + 8..offset + 12].copy_from_slice(&adjustment.to_be_bytes());
    }
    Ok(font)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_weights_map_to_nearest_theme_weight() {
        for (index, expected) in WEIGHTS.iter().enumerate() {
            assert_eq!(theme_weight((index as u16 + 1) * 100), *expected);
        }
        assert_eq!(theme_weight(0), "ultralight");
        assert_eq!(theme_weight(150), "ultralight");
        assert_eq!(theme_weight(151), "thin");
        assert_eq!(theme_weight(449), "regular");
        assert_eq!(theme_weight(450), "regular");
        assert_eq!(theme_weight(451), "medium");
        assert_eq!(theme_weight(u16::MAX), "black");
    }

    #[test]
    fn collection_preview_extracts_selected_face_and_rebuilds_checksum() {
        let mut collection = vec![0u8; 93];
        collection[..4].copy_from_slice(b"ttcf");
        collection[4..8].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        collection[8..12].copy_from_slice(&2u32.to_be_bytes());
        collection[12..16].copy_from_slice(&20u32.to_be_bytes());
        collection[16..20].copy_from_slice(&48u32.to_be_bytes());
        for offset in [20, 48] {
            collection[offset..offset + 4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
            collection[offset + 4..offset + 6].copy_from_slice(&1u16.to_be_bytes());
        }
        collection[32..36].copy_from_slice(b"name");
        collection[40..44].copy_from_slice(&76u32.to_be_bytes());
        collection[44..48].copy_from_slice(&4u32.to_be_bytes());
        collection[60..64].copy_from_slice(b"head");
        collection[68..72].copy_from_slice(&80u32.to_be_bytes());
        collection[72..76].copy_from_slice(&13u32.to_be_bytes());
        collection[80..84].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        collection[88..92].fill(0xff);
        collection[92] = 0x7f;

        let font = standalone_face(&collection, 1).unwrap();
        let raw = RawFace::parse(&font, 0).unwrap();
        assert!(ttf_parser::fonts_in_collection(&font).is_none());
        assert!(raw.table(Tag::from_bytes(b"name")).is_none());
        assert_eq!(
            raw.table(Tag::from_bytes(b"head")).unwrap()[..8],
            collection[80..88]
        );
        assert_eq!(font[40..], [0x7f, 0, 0, 0]);
        assert_eq!(checksum(&font), 0xB1B0_AFBA);
    }
}
