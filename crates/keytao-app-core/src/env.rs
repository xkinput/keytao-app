use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct AppEnv {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub resource_dir: PathBuf,
    pub app_version: String,
    pub platform: Platform,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    MacOs,
    Linux,
    Android,
    Ios,
}
