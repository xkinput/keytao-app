#!/usr/bin/env bash
# Build a smoke runtime for validating the iOS keyboard target.
#
# This is not a production librime build. It provides a tiny KeyTao FFI shim
# that commits printable ASCII directly, plus a minimal librime API stub so the
# containing iOS app can link and launch in Simulator builds.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGE_ROOT="${KEYTAO_IOS_STAGE_ROOT:-$PROJECT_DIR/target/keytao-ios-runtime}"
BUILD_ROOT="${KEYTAO_IOS_SMOKE_BUILD_ROOT:-$PROJECT_DIR/target/keytao-ios-smoke-runtime-build}"
RIME_DATA_SOURCE="${KEYTAO_IOS_SMOKE_RIME_DATA_SOURCE:-$PROJECT_DIR/vendor/librime/macos-universal/rime-data}"

TARGETS=(iphonesimulator-arm64 iphonesimulator-x86_64)

usage() {
    cat <<EOF
Usage: $0 [options]

Options:
  --target RUNTIME      Build one runtime: iphonesimulator-arm64, iphonesimulator-x86_64, or iphoneos-arm64.
  --all                 Build all simulator runtimes. This is the default.
  --stage-root DIR      Override target/keytao-ios-runtime.
  -h, --help            Show this help.

This script is for local smoke tests only. Production iOS builds must
use scripts/ios-librime-runtime.sh import-sdk and scripts/build-ios-ffi.sh.
EOF
}

die() {
    echo "ERROR: $*" >&2
    exit 1
}

note() {
    echo "==> $*"
}

clang_target_for_runtime() {
    case "$1" in
        iphonesimulator-arm64) echo "arm64-apple-ios15.0-simulator" ;;
        iphonesimulator-x86_64) echo "x86_64-apple-ios15.0-simulator" ;;
        iphoneos-arm64) echo "arm64-apple-ios15.0" ;;
        *) die "unsupported simulator runtime: $1" ;;
    esac
}

sdk_for_runtime() {
    case "$1" in
        iphonesimulator-*) echo "iphonesimulator" ;;
        iphoneos-*) echo "iphoneos" ;;
        *) die "unsupported iOS runtime: $1" ;;
    esac
}

min_version_flag_for_runtime() {
    case "$1" in
        iphonesimulator-*) echo "-mios-simulator-version-min=15.0" ;;
        iphoneos-*) echo "-miphoneos-version-min=15.0" ;;
        *) die "unsupported iOS runtime: $1" ;;
    esac
}

write_sources() {
    mkdir -p "$BUILD_ROOT"
    cat > "$BUILD_ROOT/mock_keytao_core.c" <<'EOF'
#include "keytao_core.h"

#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct KeytaoSmokeSession {
  bool ascii_mode;
} KeytaoSmokeSession;

static bool g_initialized = false;
static KeytaoSmokeSession g_singleton = { false };

static char *dup_cstr(const char *value) {
  if (!value) {
    value = "";
  }
  size_t len = strlen(value);
  char *out = (char *)malloc(len + 1);
  if (!out) {
    return NULL;
  }
  memcpy(out, value, len + 1);
  return out;
}

static char *json_escape(const char *value) {
  if (!value) {
    value = "";
  }
  size_t len = strlen(value);
  char *out = (char *)malloc(len * 6 + 1);
  if (!out) {
    return NULL;
  }
  char *cursor = out;
  for (size_t i = 0; i < len; i++) {
    unsigned char ch = (unsigned char)value[i];
    switch (ch) {
      case '"':
        *cursor++ = '\\';
        *cursor++ = '"';
        break;
      case '\\':
        *cursor++ = '\\';
        *cursor++ = '\\';
        break;
      case '\n':
        *cursor++ = '\\';
        *cursor++ = 'n';
        break;
      case '\r':
        *cursor++ = '\\';
        *cursor++ = 'r';
        break;
      case '\t':
        *cursor++ = '\\';
        *cursor++ = 't';
        break;
      default:
        if (ch < 0x20) {
          cursor += sprintf(cursor, "\\u%04x", ch);
        } else {
          *cursor++ = (char)ch;
        }
        break;
    }
  }
  *cursor = '\0';
  return out;
}

static char *state_json(KeytaoSmokeSession *session, const char *committed, bool accepted) {
  bool ascii_mode = session ? session->ascii_mode : false;
  char *escaped = json_escape(committed);
  if (!escaped) {
    return NULL;
  }
  const char *json =
      "{"
      "\"preedit\":\"\","
      "\"cursor\":0,"
      "\"candidates\":[],"
      "\"allCandidates\":[],"
      "\"highlightedCandidateIndex\":0,"
      "\"pageSize\":0,"
      "\"page\":0,"
      "\"isLastPage\":true,"
      "\"committed\":\"%s\","
      "\"selectKeys\":\"1234567890\","
      "\"asciiMode\":%s,"
      "\"schemaName\":\"KeyTao Smoke\","
      "\"accepted\":%s,"
      "\"candidatePanel\":{"
      "\"preedit\":null,"
      "\"candidates\":[],"
      "\"navigation\":{\"canGoPrevious\":false,\"canGoNext\":false}"
      "},"
      "\"modeHint\":{\"asciiMode\":%s,\"text\":\"%s\"}"
      "}";
  const char *ascii_bool = ascii_mode ? "true" : "false";
  const char *accepted_bool = accepted ? "true" : "false";
  const char *mode_text = ascii_mode ? "英" : "中";
  size_t size = strlen(json) + strlen(escaped) + strlen(mode_text) + 64;
  char *out = (char *)malloc(size);
  if (!out) {
    free(escaped);
    return NULL;
  }
  snprintf(out, size, json, escaped, ascii_bool, accepted_bool, ascii_bool, mode_text);
  free(escaped);
  return out;
}

static struct KeytaoState *state_struct(KeytaoSmokeSession *session, const char *committed, bool accepted) {
  struct KeytaoState *state = (struct KeytaoState *)calloc(1, sizeof(struct KeytaoState));
  if (!state) {
    return NULL;
  }
  state->preedit = dup_cstr("");
  state->cursor = 0;
  state->candidate_texts = NULL;
  state->candidate_comments = NULL;
  state->candidate_count = 0;
  state->highlighted_candidate_index = 0;
  state->page = 0;
  state->is_last_page = true;
  state->committed = dup_cstr(committed);
  state->select_keys = dup_cstr("1234567890");
  state->ascii_mode = session ? session->ascii_mode : false;
  state->accepted = accepted;
  return state;
}

static KeytaoSmokeSession *session_or_singleton(void *session) {
  if (session) {
    return (KeytaoSmokeSession *)session;
  }
  return &g_singleton;
}

bool keytao_init(const char *user_dir, const char *shared_dir) {
  (void)user_dir;
  (void)shared_dir;
  g_initialized = true;
  return true;
}

bool keytao_is_initialized(void) {
  return g_initialized;
}

bool keytao_reload(void) {
  return g_initialized;
}

void *keytao_create_session(void) {
  if (!g_initialized) {
    return NULL;
  }
  KeytaoSmokeSession *session = (KeytaoSmokeSession *)calloc(1, sizeof(KeytaoSmokeSession));
  return session;
}

void keytao_destroy_session(void *session) {
  free(session);
}

struct KeytaoState *keytao_session_state(void *session) {
  return state_struct(session_or_singleton(session), "", false);
}

struct KeytaoState *keytao_session_process_key(void *session, uint32_t keyval, uint32_t modifiers) {
  (void)modifiers;
  KeytaoSmokeSession *handle = session_or_singleton(session);
  char committed[8] = {0};
  bool accepted = false;
  if (keyval >= 0x20 && keyval < 0x7f) {
    committed[0] = (char)keyval;
    accepted = true;
  } else if (keyval == 0xff0d) {
    committed[0] = '\n';
    accepted = true;
  }
  return state_struct(handle, committed, accepted);
}

struct KeytaoState *keytao_session_select_candidate(void *session, uint32_t index) {
  (void)index;
  return state_struct(session_or_singleton(session), "", true);
}

struct KeytaoState *keytao_session_change_page(void *session, bool backward) {
  (void)backward;
  return state_struct(session_or_singleton(session), "", true);
}

struct KeytaoState *keytao_session_reset(void *session) {
  return state_struct(session_or_singleton(session), "", true);
}

bool keytao_session_get_ascii_mode(void *session) {
  return session_or_singleton(session)->ascii_mode;
}

struct KeytaoState *keytao_session_set_ascii_mode(void *session, bool enabled) {
  KeytaoSmokeSession *handle = session_or_singleton(session);
  handle->ascii_mode = enabled;
  return state_struct(handle, "", true);
}

void keytao_set_theme_paths(const char *default_theme_path, const char *user_theme_path) {
  (void)default_theme_path;
  (void)user_theme_path;
}

char *keytao_session_state_json(void *session) {
  return state_json(session_or_singleton(session), "", false);
}

char *keytao_session_process_key_json(void *session, uint32_t keyval, uint32_t modifiers) {
  (void)modifiers;
  KeytaoSmokeSession *handle = session_or_singleton(session);
  char committed[8] = {0};
  bool accepted = false;
  if (keyval >= 0x20 && keyval < 0x7f) {
    committed[0] = (char)keyval;
    accepted = true;
  } else if (keyval == 0xff0d) {
    committed[0] = '\n';
    accepted = true;
  } else if (keyval == 0xff08 || keyval == 0xff1b) {
    accepted = false;
  }
  return state_json(handle, committed, accepted);
}

char *keytao_session_select_candidate_json(void *session, uint32_t index) {
  (void)index;
  return state_json(session_or_singleton(session), "", true);
}

char *keytao_session_select_candidate_global_json(void *session, uint32_t index) {
  (void)index;
  return state_json(session_or_singleton(session), "", true);
}

char *keytao_session_all_candidates_json(void *session, uint32_t limit) {
  (void)session;
  (void)limit;
  return dup_cstr("[]");
}

char *keytao_session_change_page_json(void *session, bool backward) {
  (void)backward;
  return state_json(session_or_singleton(session), "", true);
}

char *keytao_session_reset_json(void *session) {
  return state_json(session_or_singleton(session), "", true);
}

char *keytao_session_set_ascii_mode_json(void *session, bool enabled) {
  KeytaoSmokeSession *handle = session_or_singleton(session);
  handle->ascii_mode = enabled;
  return state_json(handle, "", true);
}

char *keytao_resolve_theme_json(const char *default_theme_path, const char *user_theme_path) {
  (void)default_theme_path;
  (void)user_theme_path;
  return NULL;
}

char *keytao_resolve_theme_json_with_system_scheme(
  const char *default_theme_path,
  const char *user_theme_path,
  const char *system_color_scheme
) {
  (void)default_theme_path;
  (void)user_theme_path;
  (void)system_color_scheme;
  return NULL;
}

void keytao_free_string(char *ptr) {
  free(ptr);
}

struct KeytaoState *keytao_process_key(uint32_t keyval, uint32_t modifiers) {
  return keytao_session_process_key(&g_singleton, keyval, modifiers);
}

struct KeytaoState *keytao_select_candidate(uint32_t index) {
  return keytao_session_select_candidate(&g_singleton, index);
}

struct KeytaoState *keytao_change_page(bool backward) {
  return keytao_session_change_page(&g_singleton, backward);
}

struct KeytaoState *keytao_reset(void) {
  return keytao_session_reset(&g_singleton);
}

void keytao_free_state(struct KeytaoState *ptr) {
  if (!ptr) {
    return;
  }
  free(ptr->preedit);
  free(ptr->committed);
  free(ptr->select_keys);
  free(ptr->candidate_texts);
  free(ptr->candidate_comments);
  free(ptr);
}
EOF

    cat > "$BUILD_ROOT/mock_librime.c" <<'EOF'
#include "rime_api.h"

#include <stdbool.h>
#include <stddef.h>
#include <string.h>

static RimeNotificationHandler g_notification_handler = NULL;
static void *g_notification_context = NULL;
static RimeSessionId g_next_session_id = 1;

static void mock_setup(RimeTraits *traits) {
  (void)traits;
}

static void mock_set_notification_handler(RimeNotificationHandler handler, void *context_object) {
  g_notification_handler = handler;
  g_notification_context = context_object;
}

static void mock_initialize(RimeTraits *traits) {
  (void)traits;
}

static void mock_finalize(void) {}

static Bool mock_start_maintenance(Bool full_check) {
  (void)full_check;
  if (g_notification_handler) {
    g_notification_handler(g_notification_context, 0, "deploy", "success");
  }
  return True;
}

static Bool mock_is_maintenance_mode(void) {
  return False;
}

static void mock_join_maintenance_thread(void) {}

static Bool mock_success_void(void) {
  return True;
}

static RimeSessionId mock_create_session(void) {
  return g_next_session_id++;
}

static Bool mock_find_session(RimeSessionId session_id) {
  return session_id != 0;
}

static Bool mock_destroy_session(RimeSessionId session_id) {
  (void)session_id;
  return True;
}

static Bool mock_process_key(RimeSessionId session_id, int keycode, int mask) {
  (void)session_id;
  (void)keycode;
  (void)mask;
  return False;
}

static Bool mock_get_commit(RimeSessionId session_id, RimeCommit *commit) {
  (void)session_id;
  (void)commit;
  return False;
}

static Bool mock_free_commit(RimeCommit *commit) {
  (void)commit;
  return True;
}

static Bool mock_get_context(RimeSessionId session_id, RimeContext *context) {
  (void)session_id;
  (void)context;
  return False;
}

static Bool mock_free_context(RimeContext *context) {
  (void)context;
  return True;
}

static Bool mock_get_status(RimeSessionId session_id, RimeStatus *status) {
  (void)session_id;
  if (status) {
    RIME_STRUCT_INIT(RimeStatus, (*status));
    status->schema_id = "keytao";
    status->schema_name = "KeyTao Smoke";
    status->is_ascii_mode = False;
  }
  return True;
}

static Bool mock_free_status(RimeStatus *status) {
  (void)status;
  return True;
}

static void mock_set_option(RimeSessionId session_id, const char *option, Bool value) {
  (void)session_id;
  (void)option;
  (void)value;
}

static Bool mock_get_option(RimeSessionId session_id, const char *option) {
  (void)session_id;
  (void)option;
  return False;
}

static const char *mock_get_version(void) {
  return "smoke-1.0";
}

static Bool mock_select_candidate(RimeSessionId session_id, size_t index) {
  (void)session_id;
  (void)index;
  return True;
}

static Bool mock_candidate_list_begin(RimeSessionId session_id, RimeCandidateListIterator *iterator) {
  (void)session_id;
  (void)iterator;
  return False;
}

static Bool mock_candidate_list_next(RimeCandidateListIterator *iterator) {
  (void)iterator;
  return False;
}

static void mock_candidate_list_end(RimeCandidateListIterator *iterator) {
  (void)iterator;
}

static void mock_get_empty_dir(char *dir, size_t buffer_size) {
  if (dir && buffer_size > 0) {
    dir[0] = '\0';
  }
}

static Bool mock_change_page(RimeSessionId session_id, Bool backward) {
  (void)session_id;
  (void)backward;
  return True;
}

RimeApi *rime_get_api(void) {
  static RimeApi api;
  static bool initialized = false;
  if (!initialized) {
    RIME_STRUCT_INIT(RimeApi, api);
    api.setup = mock_setup;
    api.set_notification_handler = mock_set_notification_handler;
    api.initialize = mock_initialize;
    api.finalize = mock_finalize;
    api.start_maintenance = mock_start_maintenance;
    api.is_maintenance_mode = mock_is_maintenance_mode;
    api.join_maintenance_thread = mock_join_maintenance_thread;
    api.deployer_initialize = mock_setup;
    api.prebuild = mock_success_void;
    api.deploy = mock_success_void;
    api.deploy_schema = NULL;
    api.deploy_config_file = NULL;
    api.sync_user_data = mock_success_void;
    api.create_session = mock_create_session;
    api.find_session = mock_find_session;
    api.destroy_session = mock_destroy_session;
    api.process_key = mock_process_key;
    api.commit_composition = NULL;
    api.clear_composition = NULL;
    api.get_commit = mock_get_commit;
    api.free_commit = mock_free_commit;
    api.get_context = mock_get_context;
    api.free_context = mock_free_context;
    api.get_status = mock_get_status;
    api.free_status = mock_free_status;
    api.set_option = mock_set_option;
    api.get_option = mock_get_option;
    api.select_candidate = mock_select_candidate;
    api.get_version = mock_get_version;
    api.select_candidate_on_current_page = mock_select_candidate;
    api.candidate_list_begin = mock_candidate_list_begin;
    api.candidate_list_next = mock_candidate_list_next;
    api.candidate_list_end = mock_candidate_list_end;
    api.get_shared_data_dir_s = mock_get_empty_dir;
    api.get_user_data_dir_s = mock_get_empty_dir;
    api.get_prebuilt_data_dir_s = mock_get_empty_dir;
    api.get_staging_dir_s = mock_get_empty_dir;
    api.get_sync_dir_s = mock_get_empty_dir;
    api.change_page = mock_change_page;
    initialized = true;
  }
  return &api;
}

Bool RimeRegisterModule(RimeModule *module) {
  (void)module;
  return True;
}

RimeModule *RimeFindModule(const char *module_name) {
  (void)module_name;
  return NULL;
}
EOF
}

copy_rime_data() {
    local destination="$1"
    mkdir -p "$destination"
    if [ -f "$RIME_DATA_SOURCE/default.yaml" ]; then
        cp -R "$RIME_DATA_SOURCE"/. "$destination"/
    else
        cat > "$destination/default.yaml" <<'EOF'
config_version: "1.0"
schema_list:
  - schema: keytao
EOF
    fi
}

build_one() {
    local runtime="$1"
    local clang_target sdk sdkroot min_version_flag stage_dir object_dir
    clang_target="$(clang_target_for_runtime "$runtime")"
    sdk="$(sdk_for_runtime "$runtime")"
    sdkroot="$(xcrun --sdk "$sdk" --show-sdk-path)"
    min_version_flag="$(min_version_flag_for_runtime "$runtime")"
    stage_dir="$STAGE_ROOT/$runtime"
    object_dir="$BUILD_ROOT/$runtime"

    note "Building smoke runtime for $runtime"
    rm -rf "$object_dir" "$stage_dir"
    mkdir -p "$object_dir" "$stage_dir/include" "$stage_dir/lib" "$stage_dir/rime-data"

    xcrun --sdk "$sdk" clang \
        -target "$clang_target" \
        -isysroot "$sdkroot" \
        "$min_version_flag" \
        -I "$PROJECT_DIR/crates/keytao-core-ffi/include" \
        -c "$BUILD_ROOT/mock_keytao_core.c" \
        -o "$object_dir/mock_keytao_core.o"

    xcrun --sdk "$sdk" clang \
        -target "$clang_target" \
        -isysroot "$sdkroot" \
        "$min_version_flag" \
        -I "$PROJECT_DIR/vendor/librime/macos-universal/include" \
        -c "$BUILD_ROOT/mock_librime.c" \
        -o "$object_dir/mock_librime.o"

    xcrun libtool -static -o "$stage_dir/lib/libkeytao_core_ffi.a" "$object_dir/mock_keytao_core.o"
    xcrun libtool -static -o "$stage_dir/lib/librime.a" "$object_dir/mock_librime.o"

    cp "$PROJECT_DIR/crates/keytao-core-ffi/include/keytao_core.h" "$stage_dir/include/keytao_core.h"
    cp "$PROJECT_DIR/vendor/librime/macos-universal/include"/rime*.h "$stage_dir/include"/
    copy_rime_data "$stage_dir/rime-data"
    cat > "$stage_dir/librime-release.txt" <<EOF
version=smoke-1.0
source=scripts/build-ios-simulator-smoke-runtime.sh
production=false
EOF
    cat > "$stage_dir/env.sh" <<EOF
export KEYTAO_IOS_RIME_ROOT="$stage_dir"
export RIME_INCLUDE_DIR="$stage_dir/include"
export RIME_LIB_DIR="$stage_dir/lib"
export KEYTAO_RIME_SHARED_DATA_DIR="$stage_dir/rime-data"
export RIME_SHARED_DATA_DIR="$stage_dir/rime-data"
export SDKROOT="\$(xcrun --sdk $sdk --show-sdk-path)"
EOF
    note "Staged simulator smoke runtime: $stage_dir"
}

selected_targets=()

while [ "$#" -gt 0 ]; do
    case "$1" in
        --target)
            selected_targets+=("${2:-}")
            [ -n "${2:-}" ] || die "--target requires a value"
            shift 2
            ;;
        --all)
            selected_targets=("${TARGETS[@]}")
            shift
            ;;
        --stage-root)
            STAGE_ROOT="${2:-}"
            [ -n "$STAGE_ROOT" ] || die "--stage-root requires a value"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown option: $1"
            ;;
    esac
done

if [ "${#selected_targets[@]}" -eq 0 ]; then
    selected_targets=("${TARGETS[@]}")
fi

if ! command -v xcrun >/dev/null 2>&1; then
    die "xcrun is required"
fi

write_sources
for runtime in "${selected_targets[@]}"; do
    clang_target_for_runtime "$runtime" >/dev/null
    build_one "$runtime"
done

note "iOS simulator smoke runtime is ready under $STAGE_ROOT"
