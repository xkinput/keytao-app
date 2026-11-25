use reqwest::{Client, ClientBuilder};
use std::{
    collections::{HashMap, hash_map::Entry},
    sync::{LazyLock, Mutex, PoisonError},
    time::Duration,
};

#[derive(Eq, Hash, PartialEq)]
pub(crate) enum ClientProfile {
    App {
        version: String,
        follow_redirects: bool,
    },
    Addon {
        timeout: Duration,
        follow_redirects: bool,
    },
    CloudStorage,
    AppUpdate(String),
    UpdatePackage,
}

pub(crate) fn cached_client(
    profile: ClientProfile,
    builder: impl FnOnce() -> ClientBuilder,
) -> Result<Client, String> {
    static CLIENTS: LazyLock<Mutex<HashMap<ClientProfile, Client>>> = LazyLock::new(Mutex::default);
    let mut clients = CLIENTS.lock().unwrap_or_else(PoisonError::into_inner);
    match clients.entry(profile) {
        Entry::Occupied(entry) => Ok(entry.get().clone()),
        Entry::Vacant(entry) => {
            let client = builder().build().map_err(|error| error.to_string())?;
            Ok(entry.insert(client).clone())
        }
    }
}
