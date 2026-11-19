fn main() {
    let path = "../../scripts/librime-versions.env";
    println!("cargo:rerun-if-changed={path}");
    let versions = std::fs::read_to_string(path).expect("read pinned librime versions");
    let lua_ref = versions
        .lines()
        .find_map(|line| line.strip_prefix("LIBRIME_LUA_REF="))
        .expect("LIBRIME_LUA_REF must be pinned");
    assert!(
        lua_ref.len() == 40 && lua_ref.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "LIBRIME_LUA_REF must be a full commit hash"
    );
    println!(
        "cargo:rustc-env=KEYTAO_LIBRIME_LUA_VERSION={}",
        &lua_ref[..7]
    );
}
