use crate::{Core, CoreError};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, OnceLock},
    time::SystemTime,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UserWord {
    pub id: u32,
    pub word: String,
    pub code: String,
    pub weight: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UserWordComparison {
    pub entry: UserWord,
    pub status: String,
    pub words: Vec<String>,
    pub codes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UserWordMutation {
    pub already_exists: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct UserWordsReport {
    pub removed: Vec<UserWord>,
    pub conflicts: Vec<UserWordComparison>,
}

#[derive(Default)]
pub(crate) struct UserWordsState {
    reports: HashMap<String, Result<UserWordsReport, String>>,
    packages: HashMap<String, CachedPackage>,
}

#[derive(PartialEq, Eq)]
struct PackageFingerprint {
    length: u64,
    modified: Option<SystemTime>,
    metadata: Vec<u8>,
    #[cfg(unix)]
    identity: (u64, i64, i64),
}

struct CachedPackage {
    fingerprint: Option<PackageFingerprint>,
    data: Arc<PackageData>,
}

struct PackageData {
    user_name: String,
    template: Option<Vec<u8>>,
    dictionaries: HashMap<String, Vec<u8>>,
    main: Option<String>,
    index: OnceLock<Result<PackageIndex, String>>,
}

#[derive(Default)]
struct PackageIndex {
    by_word: HashMap<String, HashSet<String>>,
    by_code: HashMap<String, HashSet<String>>,
}

impl PackageIndex {
    fn build(package: &PackageData) -> Result<Self, CoreError> {
        let main = package.main.as_ref().ok_or_else(|| {
            CoreError::Other(
                "Stored package has no main dictionary; comparison is unavailable".into(),
            )
        })?;
        let mut index = Self::default();
        let mut remaining = vec![main.clone()];
        let mut visited = HashSet::new();
        while let Some(name) = remaining.pop() {
            if name.ends_with(".user") || !visited.insert(name.clone()) {
                continue;
            }
            let bytes = package_file(&package.dictionaries, &name)?.ok_or_else(|| {
                CoreError::Other(format!("Imported package dictionary is missing: {name}"))
            })?;
            let yaml = header(bytes)?;
            remaining.extend(imports(&yaml));
            for entry in Dictionary::parse(bytes)?.entries() {
                index
                    .by_word
                    .entry(entry.word.clone())
                    .or_default()
                    .insert(entry.code.clone());
                index
                    .by_code
                    .entry(entry.code)
                    .or_default()
                    .insert(entry.word);
            }
        }
        Ok(index)
    }

    fn compare(&self, entry: UserWord) -> UserWordComparison {
        let identical = self
            .by_word
            .get(&entry.word)
            .is_some_and(|codes| codes.contains(&entry.code));
        let mut words: Vec<String> = self
            .by_code
            .get(&entry.code)
            .into_iter()
            .flatten()
            .filter(|word| *word != &entry.word)
            .cloned()
            .collect();
        let mut codes: Vec<String> = self
            .by_word
            .get(&entry.word)
            .into_iter()
            .flatten()
            .filter(|code| *code != &entry.code)
            .cloned()
            .collect();
        words.sort();
        codes.sort();
        let status = if identical {
            "identical"
        } else if !words.is_empty() {
            "code_taken"
        } else if !codes.is_empty() {
            "word_has_code"
        } else {
            "none"
        };
        UserWordComparison {
            entry,
            status: status.into(),
            words,
            codes,
        }
    }
}

fn package_index(package: &PackageData) -> Result<&PackageIndex, CoreError> {
    package
        .index
        .get_or_init(|| PackageIndex::build(package).map_err(|error| error.to_string()))
        .as_ref()
        .map_err(|error| CoreError::Other(error.clone()))
}

fn header(bytes: &[u8]) -> Result<serde_yaml::Value, CoreError> {
    let text = std::str::from_utf8(bytes).map_err(|error| CoreError::Other(error.to_string()))?;
    let end = text
        .split_inclusive('\n')
        .scan(0, |offset, line| {
            *offset += line.len();
            Some((line_body(line).trim() == "...", *offset))
        })
        .find_map(|(end, offset)| end.then_some(offset))
        .unwrap_or(text.len());
    serde_yaml::from_str(text[..end].trim_start_matches('\u{feff}'))
        .map_err(|error| CoreError::Other(error.to_string()))
}

fn imports(header: &serde_yaml::Value) -> Vec<String> {
    header
        .get("import_tables")
        .and_then(|value| value.as_sequence())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\\')
        && !name.chars().any(char::is_whitespace)
        && Path::new(name)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn package_file<'a>(
    files: &'a HashMap<String, Vec<u8>>,
    name: &str,
) -> Result<Option<&'a Vec<u8>>, CoreError> {
    if let Some(bytes) = files.get(name) {
        return Ok(Some(bytes));
    }
    let suffix = format!("/{name}");
    let mut matches = files.iter().filter(|(path, _)| path.ends_with(&suffix));
    let found = matches.next().map(|(_, bytes)| bytes);
    if matches.next().is_some() {
        return Err(CoreError::Other(format!(
            "Package dictionary path is ambiguous: {name}"
        )));
    }
    Ok(found)
}

fn package_data(
    state: &mut UserWordsState,
    core: &Core,
    scheme: &str,
) -> Result<Arc<PackageData>, CoreError> {
    let scheme = match scheme {
        "keytao-dz" | "keytao-bj" => "keytao",
        _ => scheme,
    };
    crate::scheme::validate_scheme_key(scheme).map_err(CoreError::Other)?;
    let store = core.env().data_dir.join("scheme-packages");
    let path = store.join(format!("{scheme}.zip"));
    for candidate in [&store, &path] {
        if fs::symlink_metadata(candidate).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(CoreError::Other(
                "Stored package must not be a symlink".into(),
            ));
        }
    }
    let fingerprint = match fs::metadata(&path) {
        Ok(metadata) => {
            #[cfg(unix)]
            use std::os::unix::fs::MetadataExt;
            Some(PackageFingerprint {
                length: metadata.len(),
                modified: metadata.modified().ok(),
                metadata: match fs::read(store.join(format!("{scheme}.json"))) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                    Err(error) => return Err(error.into()),
                },
                #[cfg(unix)]
                identity: (metadata.ino(), metadata.ctime(), metadata.ctime_nsec()),
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if let Some(cached) = state
        .packages
        .get(scheme)
        .filter(|cached| cached.fingerprint == fingerprint)
    {
        return Ok(cached.data.clone());
    }
    let mut dictionaries = HashMap::new();
    let mut schemas = HashMap::new();
    if fingerprint.is_some() {
        let mut archive = zip::ZipArchive::new(fs::File::open(&path)?)
            .map_err(|error| CoreError::Other(error.to_string()))?;
        for index in 0..archive.len() {
            let mut file = archive
                .by_index(index)
                .map_err(|error| CoreError::Other(error.to_string()))?;
            let filename = file.name().to_owned();
            if !(filename.ends_with(".dict.yaml") || filename.ends_with(".schema.yaml")) {
                continue;
            }
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            if let Some(name) = filename.strip_suffix(".dict.yaml") {
                dictionaries.insert(name.to_owned(), bytes);
            } else if let Some(name) = filename.strip_suffix(".schema.yaml") {
                schemas.insert(name.to_owned(), bytes);
            }
        }
    }
    let base = match scheme {
        "xmjd" | "xmjd-feiding" => "xmjd6",
        _ => scheme,
    };
    let mut candidates = Vec::new();
    if let Some(schema) = package_file(&schemas, base)? {
        if let Some(name) = header(schema)?
            .get("translator")
            .and_then(|value| value.get("dictionary"))
            .and_then(|value| value.as_str())
        {
            candidates.push(name.to_owned());
        }
    }
    candidates.extend([format!("{base}.extended"), base.into()]);
    let mut main = None;
    for name in candidates {
        if package_file(&dictionaries, &name)?.is_some() {
            main = Some(name);
            break;
        }
    }
    let user_name = if let Some(name) = &main {
        imports(&header(package_file(&dictionaries, name)?.unwrap())?)
            .into_iter()
            .find(|name| name.ends_with(".user"))
    } else {
        None
    };
    let user_name = match user_name {
        Some(name) if name.contains('/') => {
            return Err(CoreError::Unsupported(
                "Nested user dictionary paths are not supported".into(),
            ));
        }
        Some(name) if safe_name(&name) => name,
        Some(_) => {
            return Err(CoreError::Other(
                "Invalid user dictionary name in package".into(),
            ));
        }
        None => fallback_dict(scheme)?.into(),
    };
    let template = package_file(&dictionaries, &user_name)?.cloned();
    let data = Arc::new(PackageData {
        user_name,
        template,
        dictionaries,
        main,
        index: OnceLock::new(),
    });
    state.packages.insert(
        scheme.into(),
        CachedPackage {
            fingerprint,
            data: data.clone(),
        },
    );
    Ok(data)
}

pub fn take_user_words_report(core: &Core, scheme: &str) -> Result<UserWordsReport, CoreError> {
    let mut state = core
        .user_words
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    state
        .reports
        .remove(scheme)
        .unwrap_or_else(|| Ok(UserWordsReport::default()))
        .map_err(CoreError::Other)
}

pub fn remember_reconciliation(core: &Core, scheme: &str, root: &Path, remove: bool) {
    let report =
        reconcile_user_words(core, scheme, root, remove).map_err(|error| error.to_string());
    core.user_words
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .reports
        .insert(scheme.into(), report);
}

pub fn compare_user_words(
    core: &Core,
    scheme: &str,
    root: &Path,
) -> Result<Vec<UserWordComparison>, CoreError> {
    let mut state = core
        .user_words
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let package = package_data(&mut state, core, scheme)?;
    let index = package_index(&package)?;
    let dictionary = Dictionary::parse(&fs::read(user_path(&package, root, true)?)?)?;
    Ok(dictionary
        .entries()
        .into_iter()
        .map(|entry| index.compare(entry))
        .collect())
}

pub fn lookup_user_word(
    core: &Core,
    scheme: &str,
    _root: &Path,
    word: &str,
    code: &str,
) -> Result<UserWordComparison, CoreError> {
    let mut state = core
        .user_words
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let package = package_data(&mut state, core, scheme)?;
    Ok(package_index(&package)?.compare(UserWord {
        id: 0,
        word: word.into(),
        code: code.into(),
        weight: None,
    }))
}

pub fn reconcile_user_words(
    core: &Core,
    scheme: &str,
    root: &Path,
    remove: bool,
) -> Result<UserWordsReport, CoreError> {
    let mut state = core
        .user_words
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let package = package_data(&mut state, core, scheme)?;
    let path = user_path(&package, root, false)?;
    if !path.try_exists()? {
        return Ok(UserWordsReport::default());
    }
    let index = package_index(&package)?;
    let mut dictionary = Dictionary::parse(&fs::read(&path)?)?;
    let entries = dictionary.entries();
    let mut code_counts = HashMap::new();
    for entry in &entries {
        *code_counts.entry(entry.code.as_str()).or_insert(0) += 1;
    }
    let removed: Vec<UserWord> = entries
        .iter()
        .filter(|entry| {
            remove
                && code_counts[entry.code.as_str()] == 1
                && index
                    .by_word
                    .get(&entry.word)
                    .is_some_and(|codes| codes.contains(&entry.code))
                && index
                    .by_code
                    .get(&entry.code)
                    .is_some_and(|words| words.iter().all(|word| word == &entry.word))
        })
        .cloned()
        .collect();
    if !removed.is_empty() {
        let ids: HashSet<usize> = removed.iter().map(|entry| entry.id as usize).collect();
        dictionary.lines = dictionary
            .lines
            .into_iter()
            .enumerate()
            .filter_map(|(id, line)| (!ids.contains(&id)).then_some(line))
            .collect();
        dictionary.write(&path)?;
    }
    let conflicts = dictionary
        .entries()
        .into_iter()
        .map(|entry| index.compare(entry))
        .filter(|comparison| matches!(comparison.status.as_str(), "code_taken" | "word_has_code"))
        .collect();
    Ok(UserWordsReport { removed, conflicts })
}

fn fallback_dict(scheme: &str) -> Result<&'static str, CoreError> {
    crate::scheme::validate_scheme_key(scheme).map_err(CoreError::Other)?;
    Ok(match scheme {
        "xmjd" | "xmjd-feiding" => "xmjd6.user",
        "txjx" => "txjx.user",
        "keydo" => {
            return Err(CoreError::Other(
                "The scheme package does not import a user dictionary".into(),
            ));
        }
        _ => "keytao.user",
    })
}

fn user_path(package: &PackageData, root: &Path, create: bool) -> Result<PathBuf, CoreError> {
    let relative = format!("{}.dict.yaml", package.user_name);
    let mut path = root.to_path_buf();
    for component in Path::new(&relative).components() {
        path.push(component);
        if fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(CoreError::Other(
                "User dictionary must not be a symlink".into(),
            ));
        }
    }
    if create && !path.try_exists()? {
        fs::create_dir_all(path.parent().unwrap())?;
        let bytes = package.template.clone().unwrap_or_else(|| {
            format!(
                "---\nname: {}\nversion: \"1\"\nsort: original\n...\n",
                package.user_name
            )
            .into_bytes()
        });
        Dictionary::parse(&bytes)?;
        let mut temporary = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        match temporary.persist_noclobber(&path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(CoreError::Io(error.error)),
        }
    }
    Ok(path)
}

fn line_body(line: &str) -> &str {
    let body = line.strip_suffix('\n').unwrap_or(line);
    body.strip_suffix('\r').unwrap_or(body)
}

fn validate_input(word: &str, code: &str, weight: Option<&str>) -> Result<(), CoreError> {
    if word.trim().is_empty() || word.contains(['\t', '\r', '\n']) {
        return Err(CoreError::Other(
            "Word must be nonempty and contain no tabs or newlines".into(),
        ));
    }
    if code.is_empty() || code.chars().any(char::is_whitespace) {
        return Err(CoreError::Other(
            "Code must be nonempty and contain no whitespace".into(),
        ));
    }
    if weight.is_some_and(|weight| weight.contains(['\t', '\r', '\n'])) {
        return Err(CoreError::Other(
            "Weight must contain no tabs or newlines".into(),
        ));
    }
    Ok(())
}

struct Dictionary {
    lines: Vec<String>,
    data_start: usize,
    columns: Vec<String>,
    trailing_newline: bool,
}

impl Dictionary {
    fn parse(bytes: &[u8]) -> Result<Self, CoreError> {
        let source =
            std::str::from_utf8(bytes).map_err(|error| CoreError::Other(error.to_string()))?;
        let lines: Vec<String> = source.split_inclusive('\n').map(String::from).collect();
        let data_start = lines
            .iter()
            .position(|line| line_body(line).trim() == "...")
            .map_or(lines.len(), |end| end + 1);
        let header = lines[..data_start].concat();
        let yaml: serde_yaml::Value = serde_yaml::from_str(header.trim_start_matches('\u{feff}'))
            .map_err(|error| CoreError::Other(error.to_string()))?;
        let columns = match yaml.get("columns") {
            Some(value) => value
                .as_sequence()
                .ok_or_else(|| CoreError::Other("Dictionary columns must be a list".into()))?
                .iter()
                .map(|value| {
                    value.as_str().map(String::from).ok_or_else(|| {
                        CoreError::Other("Dictionary column names must be strings".into())
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            None => vec!["text".into(), "code".into(), "weight".into()],
        };
        let document = Self {
            lines,
            data_start,
            columns,
            trailing_newline: source.ends_with('\n'),
        };
        document.column("text")?;
        document.column("code")?;
        Ok(document)
    }

    fn column(&self, name: &str) -> Result<usize, CoreError> {
        self.columns
            .iter()
            .position(|column| column == name)
            .ok_or_else(|| CoreError::Other(format!("Dictionary has no {name} column")))
    }

    fn entries(&self) -> Vec<UserWord> {
        let text_column = self.column("text").unwrap();
        let code_column = self.column("code").unwrap();
        let weight_column = self.columns.iter().position(|column| column == "weight");
        self.lines
            .iter()
            .enumerate()
            .skip(self.data_start)
            .filter_map(|(id, line)| {
                let body = line_body(line);
                if body.trim().is_empty() || body.trim_start().starts_with('#') {
                    return None;
                }
                let fields: Vec<&str> = body.split('\t').collect();
                let word = *fields.get(text_column)?;
                let code = *fields.get(code_column)?;
                if word.is_empty() || code.is_empty() {
                    return None;
                }
                Some(UserWord {
                    id: id.try_into().ok()?,
                    word: word.into(),
                    code: code.into(),
                    weight: weight_column
                        .and_then(|column| fields.get(column))
                        .filter(|value| !value.is_empty())
                        .map(|value| (*value).into()),
                })
            })
            .collect()
    }

    fn row(
        &self,
        original: Option<&str>,
        word: &str,
        code: &str,
        weight: Option<&str>,
    ) -> Result<String, CoreError> {
        let mut fields: Vec<String> = original
            .map(|line| line_body(line).split('\t').map(String::from).collect())
            .unwrap_or_default();
        let text_column = self.column("text")?;
        let code_column = self.column("code")?;
        let weight_column = self.columns.iter().position(|column| column == "weight");
        let width = (text_column.max(code_column) + 1)
            .max(weight.and(weight_column).map_or(0, |column| column + 1));
        fields.resize(fields.len().max(width), String::new());
        fields[text_column] = word.into();
        fields[code_column] = code.into();
        if let Some(column) = weight_column.filter(|column| *column < fields.len()) {
            fields[column] = weight.unwrap_or_default().into();
        }
        let ending = original
            .map(|line| {
                if line.ends_with("\r\n") {
                    "\r\n"
                } else if line.ends_with('\n') {
                    "\n"
                } else {
                    ""
                }
            })
            .unwrap_or_else(|| self.newline());
        let row = fields.join("\t");
        if row.trim_start().starts_with('#') {
            return Err(CoreError::Other(
                "The first dictionary column cannot begin with a comment marker".into(),
            ));
        }
        Ok(format!("{row}{ending}"))
    }

    fn newline(&self) -> &str {
        if self.lines.iter().any(|line| line.ends_with("\r\n")) {
            "\r\n"
        } else {
            "\n"
        }
    }

    fn write(&self, path: &Path) -> Result<(), CoreError> {
        let parent = path
            .parent()
            .ok_or_else(|| CoreError::Other("Dictionary has no parent directory".into()))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        if let Ok(metadata) = fs::metadata(path) {
            temporary
                .as_file()
                .set_permissions(metadata.permissions())?;
        }
        for (index, line) in self.lines.iter().enumerate() {
            let line = if index + 1 == self.lines.len()
                && !self.trailing_newline
                && line.ends_with('\n')
            {
                line_body(line)
            } else {
                line
            };
            temporary.write_all(line.as_bytes())?;
        }
        temporary.as_file().sync_all()?;
        temporary
            .persist(path)
            .map_err(|error| CoreError::Io(error.error))?;
        Ok(())
    }
}

pub fn list_user_words(core: &Core, scheme: &str, root: &Path) -> Result<Vec<UserWord>, CoreError> {
    let mut state = core
        .user_words
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let package = package_data(&mut state, core, scheme)?;
    Ok(Dictionary::parse(&fs::read(user_path(&package, root, true)?)?)?.entries())
}

pub fn add_user_word(
    core: &Core,
    scheme: &str,
    root: &Path,
    word: &str,
    code: &str,
    weight: Option<String>,
) -> Result<UserWordMutation, CoreError> {
    validate_input(word, code, weight.as_deref())?;
    let mut state = core
        .user_words
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let package = package_data(&mut state, core, scheme)?;
    let path = user_path(&package, root, true)?;
    let mut dictionary = Dictionary::parse(&fs::read(&path)?)?;
    if dictionary
        .entries()
        .iter()
        .any(|entry| entry.word == word && entry.code == code)
    {
        return Ok(UserWordMutation {
            already_exists: true,
        });
    }
    let row = dictionary.row(None, word, code, weight.as_deref())?;
    let needs_separator = dictionary.data_start == 0
        || line_body(&dictionary.lines[dictionary.data_start - 1]).trim() != "...";
    if dictionary
        .lines
        .last()
        .is_some_and(|line| !line.ends_with('\n'))
    {
        dictionary.lines.push(dictionary.newline().into());
    }
    if needs_separator {
        dictionary
            .lines
            .push(format!("...{}", dictionary.newline()));
    }
    dictionary.lines.push(row);
    dictionary.write(&path)?;
    Ok(UserWordMutation {
        already_exists: false,
    })
}

pub fn update_user_word(
    core: &Core,
    scheme: &str,
    root: &Path,
    id: u32,
    expected_word: &str,
    expected_code: &str,
    word: &str,
    code: &str,
    weight: Option<String>,
) -> Result<UserWordMutation, CoreError> {
    validate_input(word, code, weight.as_deref())?;
    let mut state = core
        .user_words
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let package = package_data(&mut state, core, scheme)?;
    let path = user_path(&package, root, true)?;
    let mut dictionary = Dictionary::parse(&fs::read(&path)?)?;
    if !dictionary
        .entries()
        .iter()
        .any(|entry| entry.id == id && entry.word == expected_word && entry.code == expected_code)
    {
        return Err(CoreError::Other("Stale user word; reload the list".into()));
    }
    dictionary.lines[id as usize] = dictionary.row(
        Some(&dictionary.lines[id as usize]),
        word,
        code,
        weight.as_deref(),
    )?;
    dictionary.write(&path)?;
    Ok(UserWordMutation {
        already_exists: false,
    })
}

pub fn delete_user_word(
    core: &Core,
    scheme: &str,
    root: &Path,
    id: u32,
    expected_word: &str,
    expected_code: &str,
) -> Result<(), CoreError> {
    let mut state = core
        .user_words
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let package = package_data(&mut state, core, scheme)?;
    let path = user_path(&package, root, true)?;
    let mut dictionary = Dictionary::parse(&fs::read(&path)?)?;
    if !dictionary
        .entries()
        .iter()
        .any(|entry| entry.id == id && entry.word == expected_word && entry.code == expected_code)
    {
        return Err(CoreError::Other("Stale user word; reload the list".into()));
    }
    dictionary.lines.remove(id as usize);
    dictionary.write(&path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppEnv, EventSink, Platform};
    use std::sync::Arc;

    struct Sink;
    impl EventSink for Sink {
        fn emit(&self, _: crate::CoreEvent) {}
    }

    fn fixture() -> (tempfile::TempDir, Arc<Core>) {
        let dir = tempfile::tempdir().unwrap();
        let core = Core::new(
            AppEnv {
                data_dir: dir.path().join("data"),
                cache_dir: dir.path().join("cache"),
                resource_dir: dir.path().join("resources"),
                app_version: "test".into(),
                platform: Platform::MacOs,
            },
            Arc::new(Sink),
        )
        .unwrap();
        (dir, core)
    }

    #[test]
    fn keytao_variants_share_the_same_user_dictionary() {
        let (dir, core) = fixture();
        let root = dir.path().join("rime");
        add_user_word(&core, "keytao-dz", &root, "自造词", "zici", None).unwrap();
        let words = list_user_words(&core, "keytao-bj", &root).unwrap();
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].word, "自造词");
        assert_eq!(list_user_words(&core, "keytao", &root).unwrap(), words);
        delete_user_word(&core, "keytao-bj", &root, words[0].id, "自造词", "zici").unwrap();
        assert!(list_user_words(&core, "keytao-dz", &root).unwrap().is_empty());
    }

    fn package(core: &Core, scheme: &str, files: &[(&str, &str)]) {
        let store = core.env().data_dir.join("scheme-packages");
        fs::create_dir_all(&store).unwrap();
        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, text) in files {
            archive
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(text.as_bytes()).unwrap();
        }
        fs::write(
            store.join(format!("{scheme}.zip")),
            archive.finish().unwrap().into_inner(),
        )
        .unwrap();
    }

    #[test]
    fn header_only_main_dictionary_supports_comparison() {
        for scheme in ["xmjd", "xmjd-feiding"] {
            let (dir, core) = fixture();
            package(
                &core,
                scheme,
                &[
                    (
                        "xmjd6.extended.dict.yaml",
                        "---\nname: xmjd6.extended\nsort: original\nimport_tables: [xmjd6.user, xmjd6.phrase]\n",
                    ),
                    (
                        "xmjd6.user.dict.yaml",
                        "---\nname: xmjd6.user\n...\nofficial\tabc\ncustom\tfresh\n",
                    ),
                    (
                        "xmjd6.phrase.dict.yaml",
                        "---\nname: xmjd6.phrase\n...\nofficial\tabc\n",
                    ),
                ],
            );
            let comparisons = compare_user_words(&core, scheme, dir.path()).unwrap();
            assert_eq!(comparisons.len(), 2);
            assert_eq!(comparisons[0].status, "identical");
            assert_eq!(comparisons[1].status, "none");
            assert_eq!(
                lookup_user_word(&core, scheme, dir.path(), "official", "abc")
                    .unwrap()
                    .status,
                "identical"
            );
            assert_eq!(
                reconcile_user_words(&core, scheme, dir.path(), true)
                    .unwrap()
                    .removed,
                [comparisons[0].entry.clone()]
            );
        }
    }

    #[test]
    fn header_only_user_dictionary_adds_separator_without_changing_header() {
        for (original, newline) in [
            (
                "# keep\n---\nname: keytao.user\ncolumns: [code, text, weight, note]\n",
                "\n",
            ),
            (
                "\u{feff}# keep\r\n---\r\nname: keytao.user\r\ncolumns: [code, text, weight, note]",
                "\r\n",
            ),
        ] {
            let (dir, core) = fixture();
            package(
                &core,
                "keytao",
                &[
                    (
                        "keytao.extended.dict.yaml",
                        "---\nname: keytao.extended\nimport_tables: [keytao.user]\n...\n",
                    ),
                    ("keytao.user.dict.yaml", original),
                ],
            );
            let path = dir.path().join("keytao.user.dict.yaml");
            assert!(
                list_user_words(&core, "keytao", dir.path())
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
            add_user_word(&core, "keytao", dir.path(), "新增", "abc", Some("8".into())).unwrap();
            let boundary = if original.ends_with('\n') {
                ""
            } else {
                newline
            };
            let ending = if original.ends_with('\n') {
                newline
            } else {
                ""
            };
            assert_eq!(
                fs::read(&path).unwrap(),
                format!("{original}{boundary}...{newline}abc\t新增\t8{ending}").as_bytes()
            );
            let entries = list_user_words(&core, "keytao", dir.path()).unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].word, "新增");
            assert_eq!(entries[0].code, "abc");
        }
    }

    #[test]
    fn reconciliation_preserves_identical_row_with_shared_package_code() {
        let (dir, core) = fixture();
        package(
            &core,
            "xmjd",
            &[
                (
                    "xmjd6.extended.dict.yaml",
                    "---\nname: xmjd6.extended\nsort: original\nimport_tables: [xmjd6.user, xmjd6.phrase]\n...\n",
                ),
                (
                    "xmjd6.phrase.dict.yaml",
                    "---\nname: xmjd6.phrase\n...\n林抠抠\tlkk\n洛可可\tlkk\n",
                ),
            ],
        );
        let path = dir.path().join("xmjd6.user.dict.yaml");
        let original = "---\nname: xmjd6.user\nsort: original\n...\n洛可可\tlkk\n";
        fs::write(&path, original).unwrap();

        let report = reconcile_user_words(&core, "xmjd", dir.path(), true).unwrap();
        assert!(report.removed.is_empty());
        assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
        let comparisons = compare_user_words(&core, "xmjd", dir.path()).unwrap();
        assert_eq!(comparisons.len(), 1);
        assert_eq!(comparisons[0].entry.word, "洛可可");
        assert_eq!(comparisons[0].entry.code, "lkk");
        assert_eq!(comparisons[0].status, "identical");
        assert_eq!(comparisons[0].words, ["林抠抠"]);
    }

    #[test]
    fn reconciliation_preserves_identical_rows_with_shared_codes() {
        let (dir, core) = fixture();
        package(
            &core,
            "xmjd",
            &[
                (
                    "xmjd6.extended.dict.yaml",
                    "---\nname: xmjd6.extended\nsort: original\nimport_tables: [xmjd6.user, xmjd6.phrase]\n...\n",
                ),
                (
                    "xmjd6.phrase.dict.yaml",
                    "---\nname: xmjd6.phrase\n...\n洛可可\tlkk\nunique\txyz\n",
                ),
            ],
        );
        let path = dir.path().join("xmjd6.user.dict.yaml");
        let original = "---\nname: xmjd6.user\nsort: original\n...\n洛可可\tlkk\n林抠抠\tlkk\n";
        fs::write(&path, original).unwrap();
        assert!(reconcile_user_words(&core, "xmjd", dir.path(), true)
            .unwrap()
            .removed
            .is_empty());
        assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
        assert_eq!(
            compare_user_words(&core, "xmjd", dir.path()).unwrap()[0].status,
            "identical"
        );
        fs::write(&path, format!("{original}unique\txyz\n")).unwrap();
        let report = reconcile_user_words(&core, "xmjd", dir.path(), true).unwrap();
        assert_eq!(report.removed.len(), 1);
        assert_eq!(report.removed[0].word, "unique");
        assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
    }

    #[test]
    fn stale_delete_preserves_shifted_dictionary() {
        let (dir, core) = fixture();
        let path = dir.path().join("keytao.user.dict.yaml");
        let header = "---\nname: keytao.user\n...\n";
        fs::write(&path, format!("{header}old\tabc\nother\txyz\n")).unwrap();
        let entry = list_user_words(&core, "keytao", dir.path())
            .unwrap()
            .remove(0);
        for inserted in ["other\tabc", "old\txyz"] {
            let shifted = format!("{header}{inserted}\nold\tabc\nother\txyz\n");
            fs::write(&path, &shifted).unwrap();
            let result = delete_user_word(
                &core,
                "keytao",
                dir.path(),
                entry.id,
                &entry.word,
                &entry.code,
            );
            assert_eq!(fs::read(&path).unwrap(), shifted.as_bytes());
            assert!(result.unwrap_err().to_string().contains("Stale user word"));
        }
    }

    #[test]
    fn stale_update_preserves_shifted_dictionary() {
        let (dir, core) = fixture();
        let path = dir.path().join("keytao.user.dict.yaml");
        let header = "---\nname: keytao.user\n...\n";
        fs::write(&path, format!("{header}old\tabc\nother\txyz\n")).unwrap();
        let entry = list_user_words(&core, "keytao", dir.path())
            .unwrap()
            .remove(0);
        for inserted in ["other\tabc", "old\txyz"] {
            let shifted = format!("{header}{inserted}\nold\tabc\nother\txyz\n");
            fs::write(&path, &shifted).unwrap();
            let result = update_user_word(
                &core,
                "keytao",
                dir.path(),
                entry.id,
                &entry.word,
                &entry.code,
                "new",
                "def",
                None,
            );
            assert_eq!(fs::read(&path).unwrap(), shifted.as_bytes());
            assert!(result.unwrap_err().to_string().contains("Stale user word"));
        }
    }

    #[test]
    fn in_place_edit_preserves_unaffected_bytes() {
        let (dir, core) = fixture();
        let path = dir.path().join("keytao.user.dict.yaml");
        fs::write(&path, b"# preserved\r\n---\r\nname: keytao.user\r\ncolumns: [text, code, weight, note]\r\n...\r\n# comment\r\n\r\nold\tabc\t20\tunknown\r\nother\txyz\t3\tkeep").unwrap();
        let entries = list_user_words(&core, "keytao", dir.path()).unwrap();
        assert_eq!(entries.len(), 2);
        update_user_word(
            &core,
            "keytao",
            dir.path(),
            entries[0].id,
            &entries[0].word,
            &entries[0].code,
            "new",
            "def",
            Some("30".into()),
        )
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"# preserved\r\n---\r\nname: keytao.user\r\ncolumns: [text, code, weight, note]\r\n...\r\n# comment\r\n\r\nnew\tdef\t30\tunknown\r\nother\txyz\t3\tkeep");
    }

    #[test]
    fn additions_respect_xmjd_and_keytao_columns() {
        let (dir, core) = fixture();
        fs::write(
            dir.path().join("xmjd6.user.dict.yaml"),
            "---\nname: xmjd6.user\ncolumns: [text, code, stem, weight]\n...\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("keytao.user.dict.yaml"),
            "---\nname: keytao.user\ncolumns: [code, weight, text]\n...\n",
        )
        .unwrap();
        add_user_word(&core, "xmjd", dir.path(), "新词", "abc", Some("25".into())).unwrap();
        add_user_word(
            &core,
            "keytao",
            dir.path(),
            "other",
            "xyz",
            Some("8".into()),
        )
        .unwrap();
        assert!(
            fs::read_to_string(dir.path().join("xmjd6.user.dict.yaml"))
                .unwrap()
                .ends_with("新词\tabc\t\t25\n")
        );
        assert!(
            fs::read_to_string(dir.path().join("keytao.user.dict.yaml"))
                .unwrap()
                .ends_with("xyz\t8\tother\n")
        );
    }

    #[test]
    fn invalid_inputs_never_change_the_dictionary() {
        let (dir, core) = fixture();
        let path = dir.path().join("keytao.user.dict.yaml");
        let original = "---\nname: keytao.user\n...\nold\tabc\n";
        fs::write(&path, original).unwrap();
        let id = list_user_words(&core, "keytao", dir.path()).unwrap()[0].id;
        for (word, code) in [
            ("", "abc"),
            ("a\tb", "abc"),
            ("a\nb", "abc"),
            ("word", ""),
            ("word", "a b"),
            ("word", "a\tb"),
            ("word", "a\rb"),
            ("word", "a\nb"),
            ("word", "a\u{2003}b"),
        ] {
            assert!(
                add_user_word(&core, "keytao", dir.path(), word, code, None).is_err(),
                "add accepted {word:?} {code:?}"
            );
            assert!(
                update_user_word(
                    &core,
                    "keytao",
                    dir.path(),
                    id,
                    "old",
                    "abc",
                    word,
                    code,
                    None
                )
                .is_err(),
                "update accepted {word:?} {code:?}"
            );
        }
        assert_eq!(fs::read_to_string(path).unwrap(), original);
    }

    #[test]
    fn resolves_user_dictionary_from_zip_imports_and_copies_template() {
        let (dir, core) = fixture();
        let template = "\u{feff}# template\r\n---\r\nname: custom.user\r\ncolumns: [code, text]\r\n...\r\nseed\t示例\r\n";
        package(
            &core,
            "keytao",
            &[
                (
                    "release/keytao.schema.yaml",
                    "schema: {schema_id: keytao}\ntranslator: {dictionary: unusual.main}\n",
                ),
                (
                    "release/unusual.main.dict.yaml",
                    "---\nname: unusual.main\nimport_tables: [custom.user]\n...\n",
                ),
                ("release/custom.user.dict.yaml", template),
            ],
        );
        let entries = list_user_words(&core, "keytao", dir.path()).unwrap();
        assert_eq!(entries[0].word, "示例");
        assert_eq!(
            fs::read(dir.path().join("custom.user.dict.yaml")).unwrap(),
            template.as_bytes()
        );
        add_user_word(&core, "keytao", dir.path(), "新增", "new", None).unwrap();
        assert!(
            fs::read_to_string(dir.path().join("custom.user.dict.yaml"))
                .unwrap()
                .ends_with("new\t新增\r\n")
        );
        assert!(!dir.path().join("keytao.user.dict.yaml").exists());
    }

    #[test]
    fn resolves_nested_dictionary_imports_inside_a_release_wrapper() {
        let (dir, core) = fixture();
        let template = "\u{feff}# nested template\n---\nname: custom.user\ncolumns: [code, text]\n...\nseed\t示例\n";
        package(
            &core,
            "keytao",
            &[
                (
                    "release/keytao.schema.yaml",
                    "schema: {schema_id: keytao}\ntranslator: {dictionary: unusual.main}\n",
                ),
                (
                    "release/unusual.main.dict.yaml",
                    "---\nname: unusual.main\nimport_tables: [custom.user, dicts/phrases]\n...\n",
                ),
                ("release/custom.user.dict.yaml", template),
                (
                    "release/dicts/phrases.dict.yaml",
                    "---\nname: dicts/phrases\n...\nofficial\tabc\n",
                ),
            ],
        );
        let entries = list_user_words(&core, "keytao", dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            fs::read(dir.path().join("custom.user.dict.yaml")).unwrap(),
            template.as_bytes()
        );
        assert_eq!(
            lookup_user_word(&core, "keytao", dir.path(), "official", "abc")
                .unwrap()
                .status,
            "identical"
        );
    }

    #[test]
    fn nested_user_dictionary_is_rejected_before_creating_files() {
        let (dir, core) = fixture();
        package(
            &core,
            "keytao",
            &[(
                "keytao.extended.dict.yaml",
                "---\nname: keytao.extended\nimport_tables: [dicts/custom.user]\n...\n",
            )],
        );
        assert!(list_user_words(&core, "keytao", dir.path()).is_err());
        assert!(!dir.path().join("dicts").exists());
    }

    #[test]
    fn ambiguous_imports_never_remove_user_entries() {
        let (dir, core) = fixture();
        package(
            &core,
            "keytao",
            &[
                (
                    "keytao.extended.dict.yaml",
                    "---\nname: keytao.extended\nimport_tables: [keytao.user, phrases]\n...\n",
                ),
                (
                    "a/phrases.dict.yaml",
                    "---\nname: phrases\n...\ncustom\tabc\n",
                ),
                (
                    "b/phrases.dict.yaml",
                    "---\nname: phrases\n...\ndifferent\txyz\n",
                ),
            ],
        );
        let path = dir.path().join("keytao.user.dict.yaml");
        let original = "---\nname: keytao.user\n...\ncustom\tabc\n";
        fs::write(&path, original).unwrap();
        assert!(reconcile_user_words(&core, "keytao", dir.path(), true).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), original);
    }

    #[test]
    fn comparison_classifies_all_four_cases_and_two_column_imports() {
        let (dir, core) = fixture();
        package(
            &core,
            "xmjd",
            &[
                (
                    "xmjd6.extended.dict.yaml",
                    "---\nname: xmjd6.extended\ncolumns: [text, code, stem, weight]\nimport_tables: [xmjd6.user, xmjd6.two, xmjd6.reordered]\n...\n",
                ),
                (
                    "xmjd6.user.dict.yaml",
                    "---\nname: xmjd6.user\n...\nidentical\tone\nnew\ttaken\nknown\tnew-code\nnew-word\tunused\n",
                ),
                (
                    "xmjd6.two.dict.yaml",
                    "---\nname: xmjd6.two\ncolumns: [text, code]\n...\nidentical\tone\nother\ttaken\nknown\tofficial\n",
                ),
                (
                    "xmjd6.reordered.dict.yaml",
                    "---\nname: xmjd6.reordered\ncolumns: [weight, code, text]\n...\n9\tsecond\tknown\n",
                ),
                (
                    "not-imported.dict.yaml",
                    "---\nname: not-imported\n...\nnew-word\tunused\n",
                ),
            ],
        );
        let comparisons = compare_user_words(&core, "xmjd", dir.path()).unwrap();
        assert_eq!(
            comparisons
                .iter()
                .map(|item| item.status.as_str())
                .collect::<Vec<_>>(),
            ["identical", "code_taken", "word_has_code", "none"]
        );
        assert_eq!(comparisons[1].words, ["other"]);
        assert_eq!(comparisons[2].codes, ["official", "second"]);
        let both = lookup_user_word(&core, "xmjd", dir.path(), "known", "taken").unwrap();
        assert_eq!(both.status, "code_taken");
        assert_eq!(both.words, ["other"]);
        assert_eq!(both.codes, ["official", "second"]);
    }

    #[test]
    fn reconciliation_removes_only_identical_entries() {
        let (dir, core) = fixture();
        package(
            &core,
            "keytao",
            &[
                (
                    "keytao.extended.dict.yaml",
                    "---\nname: keytao.extended\nimport_tables: [keytao.user, keytao.phrase]\n...\n",
                ),
                (
                    "keytao.phrase.dict.yaml",
                    "---\nname: keytao.phrase\n...\nadopted\tone\nofficial\ttaken\nknown\told-code\n",
                ),
            ],
        );
        let path = dir.path().join("keytao.user.dict.yaml");
        fs::write(&path, "# keep header\n---\nname: keytao.user\n...\n# preserve\nadopted\tone\ncustom\ttaken\nknown\tnew-code\nunique\tfresh").unwrap();
        let report = reconcile_user_words(&core, "keytao", dir.path(), true).unwrap();
        assert_eq!(
            report
                .removed
                .iter()
                .map(|entry| entry.word.as_str())
                .collect::<Vec<_>>(),
            ["adopted"]
        );
        assert_eq!(
            report
                .conflicts
                .iter()
                .map(|entry| entry.status.as_str())
                .collect::<Vec<_>>(),
            ["code_taken", "word_has_code"]
        );
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "# keep header\n---\nname: keytao.user\n...\n# preserve\ncustom\ttaken\nknown\tnew-code\nunique\tfresh"
        );
    }

    #[test]
    fn comment_leading_rows_are_rejected_without_losing_words() {
        let (dir, core) = fixture();
        let path = dir.path().join("keytao.user.dict.yaml");
        let original = "---\nname: keytao.user\n...\nold\tabc\n";
        fs::write(&path, original).unwrap();
        let id = list_user_words(&core, "keytao", dir.path()).unwrap()[0].id;
        assert!(add_user_word(&core, "keytao", dir.path(), "#tag", "abc", None).is_err());
        assert!(
            update_user_word(
                &core,
                "keytao",
                dir.path(),
                id,
                "old",
                "abc",
                " #tag",
                "abc",
                None
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        fs::write(
            &path,
            "---\nname: keytao.user\ncolumns: [code, text]\n...\n",
        )
        .unwrap();
        assert!(add_user_word(&core, "keytao", dir.path(), "word", "#abc", None).is_err());
        add_user_word(&core, "keytao", dir.path(), "#tag", "abc", None).unwrap();
        assert_eq!(
            list_user_words(&core, "keytao", dir.path()).unwrap()[0].word,
            "#tag"
        );
    }

    #[test]
    fn duplicate_add_and_delete_preserve_other_rows() {
        let (dir, core) = fixture();
        let path = dir.path().join("keytao.user.dict.yaml");
        let original = "---\nname: keytao.user\n...\n# keep\nold\tabc\nother\txyz";
        fs::write(&path, original).unwrap();
        assert!(
            add_user_word(&core, "keytao", dir.path(), "old", "abc", Some("9".into()))
                .unwrap()
                .already_exists
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        let id = list_user_words(&core, "keytao", dir.path()).unwrap()[0].id;
        delete_user_word(&core, "keytao", dir.path(), id, "old", "abc").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "---\nname: keytao.user\n...\n# keep\nother\txyz"
        );
    }

    #[test]
    fn add_and_delete_preserve_missing_final_newline() {
        for newline in ["\n", "\r\n"] {
            let (dir, core) = fixture();
            let path = dir.path().join("keytao.user.dict.yaml");
            let original = format!("---{newline}name: keytao.user{newline}...{newline}old\tabc");
            fs::write(&path, &original).unwrap();
            add_user_word(&core, "keytao", dir.path(), "new", "xyz", None).unwrap();
            let entry = list_user_words(&core, "keytao", dir.path())
                .unwrap()
                .pop()
                .unwrap();
            delete_user_word(
                &core,
                "keytao",
                dir.path(),
                entry.id,
                &entry.word,
                &entry.code,
            )
            .unwrap();
            assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
        }
    }

    #[test]
    fn reconciliation_does_not_create_a_missing_user_dictionary() {
        let (dir, core) = fixture();
        assert_eq!(
            reconcile_user_words(&core, "keytao", dir.path(), true).unwrap(),
            UserWordsReport::default()
        );
        assert!(!dir.path().join("keytao.user.dict.yaml").exists());
    }

    #[test]
    fn cached_index_handles_124000_rows_and_same_version_package_replacement() {
        let (dir, core) = fixture();
        let mut table = "---\nname: keytao.phrase\ncolumns: [text, code]\n...\n".to_owned();
        for row in 0..124_000 {
            table.push_str(&format!("word{row:06}\tcode{row:06}\n"));
        }
        let main = "---\nname: keytao.extended\nimport_tables: [keytao.user, keytao.phrase]\n...\n";
        package(
            &core,
            "keytao",
            &[
                ("keytao.extended.dict.yaml", main),
                ("keytao.phrase.dict.yaml", &table),
            ],
        );
        fs::write(
            core.env().data_dir.join("scheme-packages/keytao.json"),
            "{\"version\":\"v1\"}",
        )
        .unwrap();
        assert_eq!(
            lookup_user_word(&core, "keytao", dir.path(), "word123999", "code123999")
                .unwrap()
                .status,
            "identical"
        );
        let original = core.user_words.lock().unwrap().packages["keytao"]
            .data
            .clone();
        let original_index = package_index(&original).unwrap() as *const PackageIndex;
        for _ in 0..20 {
            assert_eq!(
                lookup_user_word(&core, "keytao", dir.path(), "new", "code123999")
                    .unwrap()
                    .words,
                ["word123999"]
            );
        }
        let reused = core.user_words.lock().unwrap().packages["keytao"]
            .data
            .clone();
        assert!(Arc::ptr_eq(&original, &reused));
        assert_eq!(
            original_index,
            package_index(&reused).unwrap() as *const PackageIndex
        );
        package(
            &core,
            "keytao",
            &[
                ("keytao.extended.dict.yaml", main),
                (
                    "keytao.phrase.dict.yaml",
                    "---\nname: keytao.phrase\n...\nreplacement\tnewcode\n",
                ),
            ],
        );
        assert_eq!(
            lookup_user_word(&core, "keytao", dir.path(), "replacement", "newcode")
                .unwrap()
                .status,
            "identical"
        );
        assert_eq!(
            lookup_user_word(&core, "keytao", dir.path(), "word123999", "code123999")
                .unwrap()
                .status,
            "none"
        );
        let replaced = core.user_words.lock().unwrap().packages["keytao"]
            .data
            .clone();
        assert!(!Arc::ptr_eq(&original, &replaced));
    }
}
