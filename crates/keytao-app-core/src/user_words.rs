use crate::{Core, CoreError};
use keytao_core::runtime_log::AppOperation;
use serde::Serialize;
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap, HashSet},
    fs,
    io::{BufRead, Write},
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
    pub remove: bool,
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
    user_name: Option<String>,
    template: Option<Vec<u8>>,
    main: Option<String>,
    index: OnceLock<Result<PackageIndex, String>>,
}

impl PackageData {
    fn user_name(&self) -> Result<&str, CoreError> {
        self.user_name
            .as_deref()
            .ok_or_else(|| CoreError::Other("该方案不支持自造词".into()))
    }

    fn user_template(&self) -> Result<Cow<'_, [u8]>, CoreError> {
        let user_name = self.user_name()?;
        Ok(match &self.template {
            Some(bytes) => Cow::Borrowed(bytes),
            None => Cow::Owned(
                format!("---\nname: {user_name}\nversion: \"1\"\nsort: original\n...\n")
                    .into_bytes(),
            ),
        })
    }
}

type DictionaryFiles = HashMap<String, Vec<u8>>;

impl PackageData {
    fn visit_stock_entries(
        &self,
        dictionaries: &DictionaryFiles,
        mut visit: impl FnMut(&str, &str),
    ) -> Result<(), CoreError> {
        let main = self.main.as_ref().ok_or_else(|| {
            CoreError::Other(
                "Stored package has no main dictionary; comparison is unavailable".into(),
            )
        })?;
        let mut remaining = vec![main.clone()];
        let mut visited = HashSet::new();
        while let Some(name) = remaining.pop() {
            if name.ends_with(".user") {
                continue;
            }
            let (path, bytes) = package_file(dictionaries, &name)?.ok_or_else(|| {
                CoreError::Other(format!("Imported package dictionary is missing: {name}"))
            })?;
            if !visited.insert(path) {
                continue;
            }
            let (yaml, body) = dictionary_header(bytes)?;
            remaining.extend(imports(&yaml));
            let columns = dictionary_columns(&yaml)?;
            let text = dictionary_column(&columns, "text")?;
            let code = dictionary_column(&columns, "code")?;
            for line in body.split_inclusive('\n') {
                if let Some(body) = entry_body(line) {
                    let mut fields = body.split('\t');
                    let (word, code) = if text < code {
                        (fields.nth(text), fields.nth(code - text - 1))
                    } else {
                        let value = fields.nth(code);
                        (fields.nth(text - code - 1), value)
                    };
                    if let (Some(word), Some(code)) = (word, code)
                        && !word.is_empty()
                        && !code.is_empty()
                    {
                        visit(word, code);
                    }
                }
            }
        }
        Ok(())
    }
}

struct StockMatches {
    by_word: HashMap<String, BTreeSet<String>>,
    by_code: HashMap<String, BTreeSet<String>>,
}

impl StockMatches {
    fn scan(
        package: &PackageData,
        dictionaries: &DictionaryFiles,
        entries: &[UserWord],
    ) -> Result<Self, CoreError> {
        let mut matches = Self {
            by_word: entries
                .iter()
                .filter(|entry| !entry.remove)
                .map(|entry| (entry.word.clone(), BTreeSet::new()))
                .collect(),
            by_code: entries
                .iter()
                .filter(|entry| !entry.remove)
                .map(|entry| (entry.code.clone(), BTreeSet::new()))
                .collect(),
        };
        package.visit_stock_entries(dictionaries, |word, code| {
            if let Some(codes) = matches.by_word.get_mut(word) {
                codes.insert(code.into());
            }
            if let Some(words) = matches.by_code.get_mut(code) {
                words.insert(word.into());
            }
        })?;
        Ok(matches)
    }

    fn compare(&self, entry: UserWord) -> UserWordComparison {
        let words = self
            .by_code
            .get(entry.code.as_str())
            .into_iter()
            .flatten()
            .map(String::as_str);
        let codes = self
            .by_word
            .get(entry.word.as_str())
            .into_iter()
            .flatten()
            .map(String::as_str);
        compare_stock(entry, words, codes)
    }
}

fn compare_stock<'a>(
    entry: UserWord,
    words: impl Iterator<Item = &'a str>,
    codes: impl Iterator<Item = &'a str>,
) -> UserWordComparison {
    if entry.remove {
        return UserWordComparison {
            entry,
            status: "removed".into(),
            words: Vec::new(),
            codes: Vec::new(),
        };
    }
    let words: Vec<_> = words
        .filter(|word| *word != entry.word)
        .map(String::from)
        .collect();
    let mut identical = false;
    let codes: Vec<_> = codes
        .filter(|code| {
            let same = *code == entry.code;
            identical |= same;
            !same
        })
        .map(String::from)
        .collect();
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

#[derive(Clone, Copy)]
struct EntryOffsets {
    word: u32,
    code: u32,
    end: u32,
}

impl EntryOffsets {
    fn entry(self, text: &str) -> (&str, &str) {
        (
            &text[self.word as usize..self.code as usize],
            &text[self.code as usize..self.end as usize],
        )
    }
}

#[derive(Default)]
struct PackageIndex {
    text: String,
    by_word: Vec<EntryOffsets>,
    by_code: Vec<u32>,
}

impl PackageIndex {
    fn build(package: &PackageData, dictionaries: &DictionaryFiles) -> Result<Self, CoreError> {
        let _operation = AppOperation::start("build_user_words_index");
        let mut index = Self::default();
        package.visit_stock_entries(dictionaries, |word, code| index.push(word, code))?;
        index.finish();
        Ok(index)
    }

    fn push(&mut self, word: &str, code: &str) {
        // The package store limits expanded files to 512 MiB.
        let start = self.text.len() as u32;
        self.text.push_str(word);
        let middle = self.text.len() as u32;
        self.text.push_str(code);
        self.by_word.push(EntryOffsets {
            word: start,
            code: middle,
            end: self.text.len() as u32,
        });
    }

    fn finish(&mut self) {
        self.by_word
            .sort_unstable_by(|left, right| left.entry(&self.text).cmp(&right.entry(&self.text)));
        self.by_word
            .dedup_by(|left, right| left.entry(&self.text) == right.entry(&self.text));
        self.by_code = (0..self.by_word.len() as u32).collect();
        self.by_code.sort_unstable_by(|&left, &right| {
            let (left_word, left_code) = self.by_word[left as usize].entry(&self.text);
            let (right_word, right_code) = self.by_word[right as usize].entry(&self.text);
            (left_code, left_word).cmp(&(right_code, right_word))
        });
        self.text.shrink_to_fit();
        self.by_word.shrink_to_fit();
    }

    fn compare(&self, entry: UserWord) -> UserWordComparison {
        let word_start = self
            .by_word
            .partition_point(|row| row.entry(&self.text).0 < entry.word.as_str());
        let word_rows = &self.by_word[word_start..];
        let word_end = word_rows.partition_point(|row| row.entry(&self.text).0 == entry.word);
        let code_start = self.by_code.partition_point(|&row| {
            self.by_word[row as usize].entry(&self.text).1 < entry.code.as_str()
        });
        let code_rows = &self.by_code[code_start..];
        let code_end = code_rows
            .partition_point(|&row| self.by_word[row as usize].entry(&self.text).1 == entry.code);
        let codes = word_rows[..word_end]
            .iter()
            .map(|row| row.entry(&self.text).1);
        let words = code_rows[..code_end]
            .iter()
            .map(|&row| self.by_word[row as usize].entry(&self.text).0);
        compare_stock(entry, words, codes)
    }
}

fn package_index<'a>(
    package: &'a PackageData,
    dictionaries: &DictionaryFiles,
) -> Result<&'a PackageIndex, CoreError> {
    package
        .index
        .get_or_init(|| {
            PackageIndex::build(package, dictionaries).map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| CoreError::Other(error.clone()))
}

fn header(bytes: &[u8]) -> Result<serde_yaml::Value, CoreError> {
    dictionary_header(bytes).map(|(yaml, _)| yaml)
}

fn dictionary_header(bytes: &[u8]) -> Result<(serde_yaml::Value, &str), CoreError> {
    let text = std::str::from_utf8(bytes).map_err(|error| CoreError::Other(error.to_string()))?;
    let end = text
        .split_inclusive('\n')
        .scan(0, |offset, line| {
            *offset += line.len();
            Some((line_body(line).trim() == "...", *offset))
        })
        .find_map(|(end, offset)| end.then_some(offset))
        .unwrap_or(text.len());
    let yaml = serde_yaml::from_str(text[..end].trim_start_matches('\u{feff}'))
        .map_err(|error| CoreError::Other(error.to_string()))?;
    Ok((yaml, &text[end..]))
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
    files: &'a DictionaryFiles,
    name: &str,
) -> Result<Option<(&'a String, &'a Vec<u8>)>, CoreError> {
    if let Some(entry) = files.get_key_value(name) {
        return Ok(Some(entry));
    }
    let suffix = format!("/{name}");
    let mut matches = files.iter().filter(|(path, _)| path.ends_with(&suffix));
    let found = matches.next();
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
    Ok(load_package_data(state, core, scheme, PackageAccess::Metadata)?.0)
}

#[derive(Clone, Copy)]
enum PackageAccess {
    Support,
    Metadata,
    Scan,
    Index,
}

fn load_package_data(
    state: &mut UserWordsState,
    core: &Core,
    scheme: &str,
    access: PackageAccess,
) -> Result<(Arc<PackageData>, DictionaryFiles), CoreError> {
    let scheme = match scheme {
        "keytao-dz" | "keytao-bj" => "keytao",
        _ => scheme,
    };
    crate::scheme::validate_scheme_key(scheme).map_err(CoreError::Other)?;
    let store = crate::package_store::PackageStore::new(core);
    let path = store.package_path(scheme);
    for candidate in [&store.root(), &path] {
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
                metadata: match store.read_metadata_bytes(scheme) {
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
    if state
        .packages
        .get(scheme)
        .is_some_and(|cached| cached.fingerprint != fingerprint)
    {
        state.packages.remove(scheme);
    }
    if let Some(cached) = state.packages.get(scheme).filter(|cached| match access {
        PackageAccess::Support | PackageAccess::Metadata => true,
        PackageAccess::Scan => false,
        PackageAccess::Index => cached.data.index.get().is_some(),
    }) {
        return Ok((cached.data.clone(), HashMap::new()));
    }
    let mut dictionaries = HashMap::new();
    let mut schemas = HashMap::new();
    if fingerprint.is_some() {
        let (files, _) = store.read_files(scheme)?;
        for (filename, content) in files {
            if !(filename.ends_with(".dict.yaml") || filename.ends_with(".schema.yaml")) {
                continue;
            }
            let mut bytes = Vec::new();
            if matches!(access, PackageAccess::Support) && filename.ends_with(".dict.yaml") {
                let mut reader = content.as_slice();
                loop {
                    let offset = bytes.len();
                    if reader.read_until(b'\n', &mut bytes)? == 0
                        || std::str::from_utf8(&bytes[offset..])
                            .is_ok_and(|line| line_body(line).trim() == "...")
                    {
                        break;
                    }
                }
            } else {
                bytes = content;
            }
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
    if let Some((_, schema)) = package_file(&schemas, base)? {
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
        imports(&header(package_file(&dictionaries, name)?.unwrap().1)?)
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
        Some(name) if safe_name(&name) => Some(name),
        Some(_) => {
            return Err(CoreError::Other(
                "Invalid user dictionary name in package".into(),
            ));
        }
        None if fingerprint.is_some() => None,
        None => fallback_dict(scheme)?.map(String::from),
    };
    let template = match &user_name {
        Some(name) => package_file(&dictionaries, name)?.map(|(_, bytes)| bytes.clone()),
        None => None,
    };
    let data = Arc::new(PackageData {
        user_name,
        template,
        main,
        index: OnceLock::new(),
    });
    if matches!(access, PackageAccess::Metadata | PackageAccess::Index) {
        state.packages.insert(
            scheme.into(),
            CachedPackage {
                fingerprint,
                data: data.clone(),
            },
        );
    }
    Ok((data, dictionaries))
}

pub(crate) fn supports_user_words(core: &Core, scheme: &str) -> Result<bool, CoreError> {
    let mut state = core
        .user_words
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    Ok(
        load_package_data(&mut state, core, scheme, PackageAccess::Support)?
            .0
            .user_name
            .is_some(),
    )
}

impl Core {
    pub fn release_user_words_stock_cache(&self) {
        self.user_words
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .packages
            .clear();
    }
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
    let _operation = AppOperation::start("reconcile_user_words");
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
    let _operation = AppOperation::start("compare_user_words");
    let mut state = core
        .user_words
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let (package, dictionaries) = load_package_data(&mut state, core, scheme, PackageAccess::Scan)?;
    if package.user_name.is_none() {
        return Ok(Vec::new());
    }
    let path = user_path(core, &package, root, false);
    let missing = path
        .as_ref()
        .is_ok_and(|path| matches!(path.try_exists(), Ok(false)));
    let entries = path.and_then(|path| {
        let bytes = if missing {
            package.user_template()?.into_owned()
        } else {
            fs::read(path)?
        };
        user_entries(&bytes, scheme, root)
    });
    // Validate the stock tables before creating a missing user dictionary.
    let mut matches = StockMatches::scan(
        &package,
        &dictionaries,
        entries.as_deref().unwrap_or_default(),
    )?;
    let entries = if missing {
        let path = user_path(core, &package, root, true)?;
        let current = user_entries(&fs::read(path)?, scheme, root)?;
        if entries.as_ref().ok() != Some(&current) {
            matches = StockMatches::scan(&package, &dictionaries, &current)?;
        }
        current
    } else {
        entries?
    };
    Ok(entries
        .into_iter()
        .map(|entry| matches.compare(entry))
        .collect())
}

pub fn lookup_user_word(
    core: &Core,
    scheme: &str,
    word: &str,
    code: &str,
) -> Result<UserWordComparison, CoreError> {
    let mut state = core
        .user_words
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let (package, dictionaries) =
        load_package_data(&mut state, core, scheme, PackageAccess::Index)?;
    package.user_name()?;
    Ok(package_index(&package, &dictionaries)?.compare(UserWord {
        id: 0,
        word: word.into(),
        code: code.into(),
        weight: None,
        remove: false,
    }))
}

pub fn search_stock_words(
    core: &Core,
    scheme: &str,
    query: &str,
) -> Result<Vec<UserWord>, CoreError> {
    require_keytao_scheme(scheme)?;
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let mut state = core
        .user_words
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let (package, dictionaries) =
        load_package_data(&mut state, core, scheme, PackageAccess::Index)?;
    let index = package_index(&package, &dictionaries)?;
    Ok(index
        .by_word
        .iter()
        .map(|row| row.entry(&index.text))
        .filter(|(word, code)| word.contains(query) || code.contains(query))
        .take(100)
        .map(|(word, code)| UserWord {
            id: 0,
            word: word.into(),
            code: code.into(),
            weight: None,
            remove: false,
        })
        .collect())
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
    let (package, dictionaries) = load_package_data(&mut state, core, scheme, PackageAccess::Scan)?;
    if package.user_name.is_none() || package.main.is_none() {
        return Ok(UserWordsReport::default());
    }
    let path = user_path(core, &package, root, false)?;
    if !path.try_exists()? {
        return Ok(UserWordsReport::default());
    }
    let dictionary = fs::read(&path)
        .map_err(CoreError::from)
        .and_then(|bytes| Dictionary::parse(&bytes));
    let entries = dictionary
        .as_ref()
        .map(Dictionary::entries)
        .unwrap_or_default();
    let matches = StockMatches::scan(&package, &dictionaries, &entries)?;
    let mut dictionary = dictionary?;
    let mut code_counts = HashMap::new();
    for entry in &entries {
        *code_counts.entry(entry.code.as_str()).or_insert(0) += 1;
    }
    let removed: Vec<UserWord> = entries
        .iter()
        .filter(|entry| {
            remove
                && code_counts[entry.code.as_str()] == 1
                && matches
                    .by_code
                    .get(entry.code.as_str())
                    .is_some_and(|words| words.len() == 1 && words.contains(&entry.word))
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
        .map(|entry| matches.compare(entry))
        .filter(|comparison| matches!(comparison.status.as_str(), "code_taken" | "word_has_code"))
        .collect();
    Ok(UserWordsReport { removed, conflicts })
}

fn fallback_dict(scheme: &str) -> Result<Option<&'static str>, CoreError> {
    crate::scheme::validate_scheme_key(scheme).map_err(CoreError::Other)?;
    Ok(match scheme {
        "xmjd" | "xmjd-feiding" => Some("xmjd6.user"),
        "txjx" => Some("txjx.user"),
        "keydo" | "snow_jiandao" => None,
        _ => Some("keytao.user"),
    })
}

fn user_path(
    core: &Core,
    package: &PackageData,
    root: &Path,
    create: bool,
) -> Result<PathBuf, CoreError> {
    let user_name = package.user_name()?;
    let relative = format!("{user_name}.dict.yaml");
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
        let bytes = package.user_template()?;
        Dictionary::parse(&bytes)?;
        if user_name == "keytao.user" {
            // A recreated dictionary cannot authorize deletions from an older instance.
            crate::account::invalidate_dictionary_bases(core, root)?;
        }
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

fn entry_body(line: &str) -> Option<&str> {
    let body = line_body(line);
    let trimmed = body.trim_start();
    (!trimmed.is_empty() && !trimmed.starts_with('#')).then_some(body)
}

fn dictionary_columns(yaml: &serde_yaml::Value) -> Result<Vec<String>, CoreError> {
    match yaml.get("columns") {
        Some(value) => value
            .as_sequence()
            .ok_or_else(|| CoreError::Other("Dictionary columns must be a list".into()))?
            .iter()
            .map(|value| {
                value.as_str().map(String::from).ok_or_else(|| {
                    CoreError::Other("Dictionary column names must be strings".into())
                })
            })
            .collect(),
        None => Ok(vec!["text".into(), "code".into(), "weight".into()]),
    }
}

fn dictionary_column(columns: &[String], name: &str) -> Result<usize, CoreError> {
    columns
        .iter()
        .position(|column| column == name)
        .ok_or_else(|| CoreError::Other(format!("Dictionary has no {name} column")))
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

fn require_keytao_scheme(scheme: &str) -> Result<(), CoreError> {
    if !matches!(scheme, "keytao" | "keytao-dz" | "keytao-bj") {
        return Err(CoreError::Unsupported(
            "Stock word removal is only supported by KeyTao schemes".into(),
        ));
    }
    Ok(())
}

fn removals_path(root: &Path) -> Result<PathBuf, CoreError> {
    let path = root.join("keytao.user.remove.txt");
    if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(CoreError::Other(
            "User word removals must not be a symlink".into(),
        ));
    }
    Ok(path)
}

pub(crate) fn read_user_word_removals(root: &Path) -> Result<Vec<UserWord>, CoreError> {
    let source = match fs::read_to_string(removals_path(root)?) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.is_empty())
        .map(|(id, line)| {
            let (word, code) = line
                .split_once('\t')
                .ok_or_else(|| CoreError::Other("Invalid user word removal format".into()))?;
            validate_input(word, code, None)?;
            Ok(UserWord {
                id: id
                    .try_into()
                    .map_err(|_| CoreError::Other("Too many user word removals".into()))?,
                word: word.into(),
                code: code.into(),
                weight: None,
                remove: true,
            })
        })
        .collect()
}

pub(crate) fn write_user_word_removals(root: &Path, entries: &[UserWord]) -> Result<(), CoreError> {
    let path = removals_path(root)?;
    prepare_user_word_removals(root, &path, entries)?
        .persist(&path)
        .map_err(|error| CoreError::Io(error.error))?;
    Ok(())
}

fn prepare_user_word_removals(
    root: &Path,
    path: &Path,
    entries: &[UserWord],
) -> Result<tempfile::NamedTempFile, CoreError> {
    let mut removals = BTreeSet::new();
    for entry in entries {
        validate_input(&entry.word, &entry.code, None)?;
        if !entry.remove {
            return Err(CoreError::Other("Expected a user word removal".into()));
        }
        removals.insert((&entry.word, &entry.code));
    }
    fs::create_dir_all(root)?;
    let mut temporary = tempfile::NamedTempFile::new_in(root)?;
    if let Ok(metadata) = fs::metadata(path) {
        temporary
            .as_file()
            .set_permissions(metadata.permissions())?;
    }
    for (word, code) in removals {
        writeln!(temporary, "{word}\t{code}")?;
    }
    temporary.as_file().sync_all()?;
    Ok(temporary)
}

pub fn set_user_word_removed(
    core: &Core,
    scheme: &str,
    root: &Path,
    word: &str,
    code: &str,
    remove: bool,
) -> Result<(), CoreError> {
    let mut state = core
        .user_words
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    set_user_word_removed_locked(core, &mut state, scheme, root, word, code, remove)
}

pub(crate) fn set_user_word_removed_locked(
    core: &Core,
    state: &mut UserWordsState,
    scheme: &str,
    root: &Path,
    word: &str,
    code: &str,
    remove: bool,
) -> Result<(), CoreError> {
    require_keytao_scheme(scheme)?;
    validate_input(word, code, None)?;
    let package = package_data(state, core, scheme)?;
    user_path(core, &package, root, true)?;
    let mut entries = read_user_word_removals(root)?;
    let exists = entries
        .iter()
        .any(|entry| entry.word == word && entry.code == code);
    if exists == remove {
        return Ok(());
    }
    if remove {
        crate::account::ensure_user_dictionary_imports(root).map_err(CoreError::Other)?;
        entries.push(UserWord {
            id: 0,
            word: word.into(),
            code: code.into(),
            weight: None,
            remove: true,
        });
    } else {
        entries.retain(|entry| entry.word != word || entry.code != code);
    }
    write_user_word_removals(root, &entries)
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
        let columns = dictionary_columns(&yaml)?;
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
        dictionary_column(&self.columns, name)
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
                let body = entry_body(line)?;
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
                    remove: false,
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
        newline: &str,
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
            .unwrap_or(newline);
        let row = fields.join("\t");
        if row.trim_start().starts_with('#') {
            return Err(CoreError::Other(
                "The first dictionary column cannot begin with a comment marker".into(),
            ));
        }
        Ok(format!("{row}{ending}"))
    }

    fn newline(&self) -> &'static str {
        if self.lines.iter().any(|line| line.ends_with("\r\n")) {
            "\r\n"
        } else {
            "\n"
        }
    }

    fn append(
        &mut self,
        word: &str,
        code: &str,
        weight: Option<&str>,
        newline: &str,
    ) -> Result<(), CoreError> {
        let row = self.row(None, word, code, weight, newline)?;
        if self.lines.last().is_some_and(|line| !line.ends_with('\n')) {
            self.lines.push(newline.into());
        }
        if self.data_start == 0 || line_body(&self.lines[self.data_start - 1]).trim() != "..." {
            self.lines.push(format!("...{newline}"));
            self.data_start = self.lines.len();
        }
        self.lines.push(row);
        Ok(())
    }

    fn write(&self, path: &Path) -> Result<(), CoreError> {
        self.prepare_write(path)?
            .persist(path)
            .map_err(|error| CoreError::Io(error.error))?;
        Ok(())
    }

    fn prepare_write(&self, path: &Path) -> Result<tempfile::NamedTempFile, CoreError> {
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
        Ok(temporary)
    }
}

pub fn list_user_words(core: &Core, scheme: &str, root: &Path) -> Result<Vec<UserWord>, CoreError> {
    let mut state = core
        .user_words
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    read_user_words_locked(core, &mut state, scheme, root).map(|(_, entries)| entries)
}

pub(crate) fn read_user_words_locked(
    core: &Core,
    state: &mut UserWordsState,
    scheme: &str,
    root: &Path,
) -> Result<(Vec<u8>, Vec<UserWord>), CoreError> {
    let package = package_data(state, core, scheme)?;
    if package.user_name.is_none() {
        return Ok((Vec::new(), Vec::new()));
    }
    let content = fs::read(user_path(core, &package, root, true)?)?;
    let entries = user_entries(&content, scheme, root)?;
    Ok((content, entries))
}

fn user_entries(bytes: &[u8], scheme: &str, root: &Path) -> Result<Vec<UserWord>, CoreError> {
    let mut entries = Dictionary::parse(bytes)?.entries();
    if matches!(scheme, "keytao" | "keytao-dz" | "keytao-bj") {
        entries.extend(read_user_word_removals(root)?);
    }
    Ok(entries)
}

pub(crate) fn apply_batch_locked<'a>(
    core: &Core,
    state: &mut UserWordsState,
    root: &Path,
    current: &[u8],
    mut markers: Vec<UserWord>,
    deletes: impl Iterator<Item = &'a UserWord>,
    adds: impl Iterator<Item = UserWord>,
) -> Result<(), CoreError> {
    let (marker_deletes, word_deletes): (Vec<_>, Vec<_>) = deletes.partition(|entry| entry.remove);
    let (marker_adds, word_adds): (Vec<_>, Vec<_>) = adds.partition(|entry| entry.remove);
    if marker_deletes.is_empty()
        && marker_adds.is_empty()
        && word_deletes.is_empty()
        && word_adds.is_empty()
    {
        return Ok(());
    }
    let package = package_data(state, core, "keytao")?;
    let path = user_path(core, &package, root, false)?;
    let words = if word_deletes.is_empty() && word_adds.is_empty() {
        None
    } else {
        let mut dictionary = if path == root.join(crate::account::USER_DICTIONARY) {
            Dictionary::parse(current)?
        } else {
            Dictionary::parse(&fs::read(&path)?)?
        };
        let mut entries: HashMap<_, _> = dictionary
            .entries()
            .into_iter()
            .map(|entry| (entry.id, (entry.word, entry.code)))
            .collect();
        let mut deleted = HashSet::new();
        for entry in word_deletes {
            if !entries
                .remove(&entry.id)
                .is_some_and(|(word, code)| word == entry.word && code == entry.code)
            {
                return Err(CoreError::Other("Stale user word; reload the list".into()));
            }
            deleted.insert(entry.id as usize);
        }
        let mut line_id = 0;
        dictionary.lines.retain(|_| {
            let keep = !deleted.contains(&line_id);
            line_id += 1;
            keep
        });
        let newline = dictionary.newline();
        let mut existing: HashSet<_> = entries.into_values().collect();
        let mut changed = !deleted.is_empty();
        for entry in word_adds {
            let weight = entry.weight.as_deref();
            validate_input(&entry.word, &entry.code, weight)?;
            if existing.insert((entry.word.clone(), entry.code.clone())) {
                dictionary.append(&entry.word, &entry.code, weight, newline)?;
                changed = true;
            }
        }
        changed
            .then(|| dictionary.prepare_write(&path))
            .transpose()?
    };
    let markers = if marker_deletes.is_empty() && marker_adds.is_empty() {
        None
    } else {
        let path = removals_path(root)?;
        let deleted: HashSet<_> = marker_deletes
            .into_iter()
            .map(|entry| (entry.word.as_str(), entry.code.as_str()))
            .collect();
        markers.retain(|entry| !deleted.contains(&(entry.word.as_str(), entry.code.as_str())));
        markers.extend(marker_adds);
        Some((prepare_user_word_removals(root, &path, &markers)?, path))
    };
    if let Some(words) = words {
        words
            .persist(&path)
            .map_err(|error| CoreError::Io(error.error))?;
    }
    if let Some((markers, path)) = markers {
        markers
            .persist(&path)
            .map_err(|error| CoreError::Io(error.error))?;
    }
    Ok(())
}

pub fn add_user_word(
    core: &Core,
    scheme: &str,
    root: &Path,
    word: &str,
    code: &str,
    weight: Option<String>,
) -> Result<UserWordMutation, CoreError> {
    let mut state = core
        .user_words
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    add_user_word_locked(core, &mut state, scheme, root, word, code, weight)
}

pub(crate) fn add_user_word_locked(
    core: &Core,
    state: &mut UserWordsState,
    scheme: &str,
    root: &Path,
    word: &str,
    code: &str,
    weight: Option<String>,
) -> Result<UserWordMutation, CoreError> {
    validate_input(word, code, weight.as_deref())?;
    let package = package_data(state, core, scheme)?;
    let path = user_path(core, &package, root, true)?;
    let mut dictionary = Dictionary::parse(&fs::read(&path)?)?;
    if matches!(scheme, "keytao" | "keytao-dz" | "keytao-bj") {
        crate::account::ensure_user_dictionary_imports(root).map_err(CoreError::Other)?;
    }
    if dictionary
        .entries()
        .iter()
        .any(|entry| entry.word == word && entry.code == code)
    {
        return Ok(UserWordMutation {
            already_exists: true,
        });
    }
    dictionary.append(word, code, weight.as_deref(), dictionary.newline())?;
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
    let path = user_path(core, &package, root, true)?;
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
        "",
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
    delete_user_word_locked(
        core,
        &mut state,
        scheme,
        root,
        id,
        expected_word,
        expected_code,
    )
}

pub(crate) fn delete_user_word_locked(
    core: &Core,
    state: &mut UserWordsState,
    scheme: &str,
    root: &Path,
    id: u32,
    expected_word: &str,
    expected_code: &str,
) -> Result<(), CoreError> {
    let package = package_data(state, core, scheme)?;
    let path = user_path(core, &package, root, true)?;
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
        fs::create_dir_all(&root).unwrap();
        for name in ["keytao", "keytao-dz", "keytao-bj", "xmjd6"] {
            fs::write(
                root.join(format!("{name}.extended.dict.yaml")),
                format!("name: {name}.extended\nimport_tables:\n  - {name}.phrase\n"),
            )
            .unwrap();
        }
        add_user_word(&core, "keytao-dz", &root, "自造词", "zici", None).unwrap();
        for name in ["keytao", "keytao-dz", "keytao-bj", "xmjd6"] {
            let content =
                fs::read_to_string(root.join(format!("{name}.extended.dict.yaml"))).unwrap();
            assert_eq!(content.contains("- keytao.user"), name != "xmjd6");
        }
        let words = list_user_words(&core, "keytao-bj", &root).unwrap();
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].word, "自造词");
        assert_eq!(list_user_words(&core, "keytao", &root).unwrap(), words);
        delete_user_word(&core, "keytao-bj", &root, words[0].id, "自造词", "zici").unwrap();
        assert!(list_user_words(&core, "keytao-dz", &root).unwrap().is_empty());
    }

    fn package(core: &Core, scheme: &str, files: &[(&str, &str)]) {
        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, text) in files {
            archive
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(text.as_bytes()).unwrap();
        }
        let bytes = archive.finish().unwrap().into_inner();
        crate::packages::store_cloud_archive(core, scheme, Some("v1"), "official", None, &bytes, &core.env().data_dir).unwrap();
    }

    #[test]
    fn removal_file_is_plain_word_code_pairs_and_validated_without_lua() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keytao.user.remove.txt");
        assert!(read_user_word_removals(dir.path()).unwrap().is_empty());
        fs::write(&path, "词语\tciyu\r\n恢复\thuif").unwrap();
        let entries = read_user_word_removals(dir.path()).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .all(|entry| entry.remove && entry.weight.is_none())
        );
        assert_eq!(
            (&entries[0].word[..], &entries[0].code[..]),
            ("词语", "ciyu")
        );
        write_user_word_removals(dir.path(), &[entries.clone(), entries].concat()).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "恢复\thuif\n词语\tciyu\n"
        );
        for invalid in ["word", "word\t", "\tcode", "word\ta b", "word\tcode\t1"] {
            fs::write(&path, invalid).unwrap();
            assert!(read_user_word_removals(dir.path()).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn removals_coexist_with_personal_words_and_can_be_restored_across_variants() {
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
                    "---\nname: keytao.phrase\n...\nother\txyz\n",
                ),
            ],
        );
        add_user_word(&core, "keytao", dir.path(), "shared", "abc", None).unwrap();
        let dictionary = dir.path().join("keytao.user.dict.yaml");
        let before = fs::read(&dictionary).unwrap();
        set_user_word_removed(&core, "keytao-dz", dir.path(), "shared", "abc", true).unwrap();
        set_user_word_removed(&core, "keytao-bj", dir.path(), "shared", "abc", true).unwrap();
        let entries = list_user_words(&core, "keytao-bj", dir.path()).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(!entries[0].remove);
        assert!(entries[1].remove);
        assert_eq!(entries[0].word, entries[1].word);
        assert_eq!(entries[0].code, entries[1].code);
        assert_eq!(fs::read(&dictionary).unwrap(), before);
        let comparisons = compare_user_words(&core, "keytao", dir.path()).unwrap();
        assert_eq!(comparisons[1].status, "removed");
        assert!(
            reconcile_user_words(&core, "keytao", dir.path(), true)
                .unwrap()
                .removed
                .is_empty()
        );
        assert_eq!(read_user_word_removals(dir.path()).unwrap().len(), 1);
        set_user_word_removed(&core, "keytao", dir.path(), "shared", "abc", false).unwrap();
        assert_eq!(
            list_user_words(&core, "keytao-dz", dir.path())
                .unwrap()
                .len(),
            1
        );
        assert_eq!(fs::read(&dictionary).unwrap(), before);
        assert_eq!(
            fs::read_to_string(dir.path().join("keytao.user.remove.txt")).unwrap(),
            ""
        );
    }

    #[test]
    fn removals_are_keytao_only_and_reject_invalid_input_without_writing() {
        let (dir, core) = fixture();
        for scheme in ["xmjd", "xmjd-feiding", "txjx", "local-example", "../keytao"] {
            assert!(set_user_word_removed(&core, scheme, dir.path(), "word", "abc", true).is_err());
        }
        for (word, code) in [("", "abc"), ("a\tb", "abc"), ("word", "a b")] {
            assert!(set_user_word_removed(&core, "keytao", dir.path(), word, code, true).is_err());
        }
        assert!(!dir.path().join("keytao.user.remove.txt").exists());
        assert!(!dir.path().join("keytao.user.dict.yaml").exists());
    }

    #[test]
    fn stock_search_uses_the_package_index_and_excludes_user_dictionary() {
        let (_dir, core) = fixture();
        let mut table = "---\nname: keytao.phrase\n...\n".to_owned();
        for row in (0..105).rev() {
            table.push_str(&format!("word{row:03}\tcode{row:03}\n"));
        }
        package(
            &core,
            "keytao",
            &[
                (
                    "keytao.extended.dict.yaml",
                    "---\nname: keytao.extended\nimport_tables: [keytao.user, keytao.phrase]\n...\n",
                ),
                ("keytao.phrase.dict.yaml", &table),
                (
                    "keytao.user.dict.yaml",
                    "---\nname: keytao.user\n...\npersonal\tprivate\n",
                ),
            ],
        );
        let entries = search_stock_words(&core, "keytao-dz", "word").unwrap();
        assert_eq!(entries.len(), 100);
        assert_eq!(entries.first().unwrap().word, "word000");
        assert_eq!(entries.last().unwrap().word, "word099");
        assert!(entries.iter().all(|entry| !entry.remove));
        assert_eq!(
            search_stock_words(&core, "keytao-bj", "code104").unwrap()[0].word,
            "word104"
        );
        assert!(
            search_stock_words(&core, "keytao", "personal")
                .unwrap()
                .is_empty()
        );
        assert!(search_stock_words(&core, "keytao", " ").unwrap().is_empty());
        assert!(search_stock_words(&core, "xmjd", "word").is_err());
    }

    #[test]
    fn reconciliation_and_comparison_match_lookup_without_caching_stock_entries() {
        let (dir, core) = fixture();
        package(
            &core,
            "keytao",
            &[
                (
                    "keytao.extended.dict.yaml",
                    "---\nname: keytao.extended\nimport_tables: [keytao.user, stock, extra]\n...\n",
                ),
                (
                    "nested/stock.dict.yaml",
                    "\u{feff}---\r\nname: stock\r\ncolumns: [code, weight, text]\r\n...\r\n  # comment\r\nab\t8\t目标\r\naz\t2\t目标\r\nab\t\talpha\r\nab\t\t\r\nsolo\t1\tunique",
                ),
                (
                    "extra.dict.yaml",
                    "---\nname: extra\nimport_tables: [nested/stock]\n...\n目标\tab\nthird\txaby\n",
                ),
            ],
        );
        fs::write(
            dir.path().join("keytao.user.dict.yaml"),
            "---\nname: keytao.user\n...\nunique\tsolo\n目标\tab\nnew\tab\n目标\tnew\n",
        )
        .unwrap();
        remember_reconciliation(&core, "keytao", dir.path(), true);
        let report = take_user_words_report(&core, "keytao").unwrap();
        assert_eq!(report.removed[0].word, "unique");
        assert!(core.user_words.lock().unwrap().packages.is_empty());

        let comparisons = compare_user_words(&core, "keytao-bj", dir.path()).unwrap();
        assert!(core.user_words.lock().unwrap().packages.is_empty());
        assert_eq!(comparisons[0].entry.id, 3);
        assert_eq!(comparisons[0].words, ["alpha"]);
        assert_eq!(comparisons[0].codes, ["az"]);
        assert_eq!(report.conflicts, comparisons[1..]);
        for mut comparison in comparisons {
            comparison.entry.id = 0;
            assert_eq!(
                lookup_user_word(
                    &core,
                    "keytao-dz",
                    &comparison.entry.word,
                    &comparison.entry.code
                )
                .unwrap(),
                comparison
            );
        }
        let results = search_stock_words(&core, "keytao", " ab ").unwrap();
        assert_eq!(
            results
                .iter()
                .map(|entry| entry.word.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "third", "目标"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn removal_file_rejects_symlinks() {
        let (dir, core) = fixture();
        let outside = dir.path().join("outside.txt");
        fs::write(&outside, "outside\tabc\n").unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join("keytao.user.remove.txt")).unwrap();
        assert!(set_user_word_removed(&core, "keytao", dir.path(), "word", "xyz", true).is_err());
        assert!(read_user_word_removals(dir.path()).is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "outside\tabc\n");
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
                lookup_user_word(&core, scheme, "official", "abc")
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
            lookup_user_word(&core, "keytao", "official", "abc")
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
        assert!(compare_user_words(&core, "keytao", dir.path()).is_err());
        assert!(!path.exists());
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
        let both = lookup_user_word(&core, "xmjd", "known", "taken").unwrap();
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
    fn reconciliation_skips_packages_without_a_user_dictionary() {
        for scheme in ["keydo", "keytao", "local-plain"] {
            let (dir, core) = fixture();
            package(
                &core,
                scheme,
                &[
                    (
                        &format!("{scheme}.schema.yaml"),
                        &format!(
                            "translator:\n  dictionary: {scheme}\n  enable_user_dict: false\n"
                        ),
                    ),
                    (
                        &format!("{scheme}.dict.yaml"),
                        &format!("---\nname: {scheme}\n...\nofficial\tabc\n"),
                    ),
                ],
            );
            let path = dir.path().join("keytao.user.dict.yaml");
            let original = "---\nname: keytao.user\n...\nofficial\tabc\n";
            fs::write(&path, original).unwrap();

            assert!(!supports_user_words(&core, scheme).unwrap());
            assert!(
                list_user_words(&core, scheme, dir.path())
                    .unwrap()
                    .is_empty()
            );
            assert!(
                compare_user_words(&core, scheme, dir.path())
                    .unwrap()
                    .is_empty()
            );
            for remove in [false, true] {
                assert_eq!(
                    reconcile_user_words(&core, scheme, dir.path(), remove).unwrap(),
                    UserWordsReport::default()
                );
                remember_reconciliation(&core, scheme, dir.path(), remove);
                assert_eq!(
                    take_user_words_report(&core, scheme).unwrap(),
                    UserWordsReport::default()
                );
                assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
            }
            assert_eq!(
                add_user_word(&core, scheme, dir.path(), "custom", "xyz", None)
                    .unwrap_err()
                    .to_string(),
                "该方案不支持自造词"
            );
            assert_eq!(
                update_user_word(
                    &core,
                    scheme,
                    dir.path(),
                    4,
                    "official",
                    "abc",
                    "custom",
                    "xyz",
                    None,
                )
                .unwrap_err()
                .to_string(),
                "该方案不支持自造词"
            );
            assert_eq!(
                delete_user_word(&core, scheme, dir.path(), 4, "official", "abc")
                    .unwrap_err()
                    .to_string(),
                "该方案不支持自造词"
            );
            assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
        }
    }

    #[test]
    fn reconciliation_skips_packages_without_a_main_dictionary() {
        let (dir, core) = fixture();
        let scheme = "local-mytest";
        package(
            &core,
            scheme,
            &[(
                "local-mytest.schema.yaml",
                "schema:\n  schema_id: local-mytest\n  name: My Test\n",
            )],
        );
        let path = dir.path().join("keytao.user.dict.yaml");
        let original = "---\nname: keytao.user\n...\ncustom\tabc\n";
        fs::write(&path, original).unwrap();

        for remove in [false, true] {
            remember_reconciliation(&core, scheme, dir.path(), remove);
            assert_eq!(
                take_user_words_report(&core, scheme).unwrap(),
                UserWordsReport::default()
            );
            assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
        }
        assert!(!supports_user_words(&core, scheme).unwrap());
        assert!(
            compare_user_words(&core, scheme, dir.path())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cached_index_handles_124000_rows_and_same_version_package_replacement() {
        let (_dir, core) = fixture();
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
        assert_eq!(
            lookup_user_word(&core, "keytao", "word123999", "code123999")
                .unwrap()
                .status,
            "identical"
        );
        let original = core.user_words.lock().unwrap().packages["keytao"]
            .data
            .clone();
        let original_index = original.index.get().unwrap().as_ref().unwrap() as *const PackageIndex;
        for _ in 0..20 {
            assert_eq!(
                lookup_user_word(&core, "keytao", "new", "code123999")
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
            reused.index.get().unwrap().as_ref().unwrap() as *const PackageIndex
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
            lookup_user_word(&core, "keytao", "replacement", "newcode")
                .unwrap()
                .status,
            "identical"
        );
        assert_eq!(
            lookup_user_word(&core, "keytao", "word123999", "code123999")
                .unwrap()
                .status,
            "none"
        );
        let replaced = core.user_words.lock().unwrap().packages["keytao"]
            .data
            .clone();
        assert!(!Arc::ptr_eq(&original, &replaced));
        core.release_user_words_stock_cache();
        assert!(core.user_words.lock().unwrap().packages.is_empty());
        assert_eq!(
            lookup_user_word(&core, "keytao", "replacement", "newcode")
                .unwrap()
                .status,
            "identical"
        );
    }
}
