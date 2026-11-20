#ifndef KEYTAO_CORE_H
#define KEYTAO_CORE_H

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * Bit 0. `RimeSelectCandidateOnCurrentPage` is there: tapping a candidate
 * selects it. Without this bit a tap can only send the schema's
 * `menu/select_keys` character, which schemas without select keys type into
 * the composition instead — so the frontend must not offer tap-to-select.
 */
#define KEYTAO_CAP_CANDIDATE_SELECTION 1

/**
 * Bit 1. `RimeSelectCandidate`: selection by index into the whole candidate
 * list, what keytao_session_select_candidate_global_json() needs.
 */
#define KEYTAO_CAP_GLOBAL_CANDIDATE_SELECTION 2

/**
 * Bit 2. `RimeHighlightCandidateOnCurrentPage`: moving the highlight without
 * committing. Without this bit hover and arrow-key highlighting do nothing.
 */
#define KEYTAO_CAP_CANDIDATE_HIGHLIGHT 4

/**
 * Bit 3. `RimeDeleteCandidateOnCurrentPage`, the "forget this phrase" gesture.
 */
#define KEYTAO_CAP_CANDIDATE_DELETION 8

/**
 * Bit 4. `RimeChangePage`: paging through librime. Without this bit paging
 * replays the `-`/`=` bindings of the default `key_binder` preset, which many
 * schemas never import — so the frontend must not offer page up/down controls.
 */
#define KEYTAO_CAP_NATIVE_PAGING 16

/**
 * Bit 5. `RimeCommitComposition`. Without it, committing sends `Return`.
 */
#define KEYTAO_CAP_COMMIT_COMPOSITION 32

/**
 * Bit 6. `RimeClearComposition`. Without it, discarding sends `Escape`.
 */
#define KEYTAO_CAP_CLEAR_COMPOSITION 64

/**
 * Whether a runtime log level is enabled (0 = off, 1 = info, 2 = verbose).
 * Invalid levels are disabled.
 */
bool keytao_log_enabled(int32_t level);

/**
 * Record a runtime event. `dur_ms` may be NaN to omit the duration;
 * `kv_json` may be null, otherwise it must be a UTF-8 JSON object.
 * `cat` and `ev` must be non-null, null-terminated UTF-8 strings.
 * Fields must contain only metadata, never typed, candidate or committed text.
 */
void keytao_log_event(int32_t level,
                      const char *cat,
                      const char *ev,
                      double dur_ms,
                      const char *kv_json);

void keytao_log_keystroke(double ms);

void keytao_log_drain_key_stats(void);

/**
 * Wait up to `timeout_ms` for queued runtime log events to be written, without fsync.
 */
void keytao_log_flush(uint32_t timeout_ms);

/**
 * Run the macOS executable's `--deploy` entry without initializing a session.
 * Terminates the process without running librime's static destructors.
 */
void keytao_run_deploy_child_main(void);

/**
 * Run the macOS executable's read-only `--preview-candidates` entry.
 * Terminates the process without running librime's static destructors.
 */
void keytao_run_preview_child_main(void);

/**
 * Redeploy through the macOS executable itself, with a 600-second timeout.
 * Returns true only for a successful exit and a valid success protocol result.
 */
bool keytao_run_deploy_child(void);

/**
 * Initialize the Rime runtime. Must be called once before any other function.
 * Both `user_dir` and `shared_dir` must be non-null UTF-8 strings.
 * Returns true on success.
 *
 * Calls are serialized and a repeat for the directories already running is a
 * no-op, so a frontend that initializes from two paths at once keeps the
 * session it is composing in. Switching to different directories takes
 * librime down and back up: sessions created before the switch stop answering
 * and have to be recreated with keytao_create_session().
 */
bool keytao_init(const char *user_dir, const char *shared_dir);

bool keytao_is_initialized(void);

/**
 * Redeploy Rime data through the shared runtime. Every session created through
 * this library drops its engine first, so librime re-reads the deployment
 * instead of serving its cached config and dictionaries.
 */
bool keytao_reload(void);

/**
 * Path of the reload signal inside `user_dir`, for frontends that watch the
 * file before keytao_init() has succeeded. Free with keytao_free_string().
 */
char *keytao_reload_stamp_path_at(const char *user_dir);

/**
 * Reload when the app requested one, returning true only if a reload actually
 * ran. Cheap enough for a timer, but not for the key path: it stats a file.
 */
bool keytao_reload_if_stamp_changed(void);

/**
 * Cached shared key gate, including failed initialization before a session exists.
 */
bool keytao_deployment_blocks_input(void);

/**
 * Whether the deployment sentinel belongs to a live deployer. This reads the filesystem.
 */
bool keytao_deployment_in_progress_at(const char *user_dir);

/**
 * Read the shared deployment hint off the key thread. Free with keytao_free_string().
 * Returns null once the transient hint expires.
 */
char *keytao_deployment_hint(void);

/**
 * Record a key before waiting for deployment or engine rebuilding.
 */
void keytao_deployment_key_pressed(void);

/**
 * Clear deployment presentation when focus or the input session changes.
 */
void keytao_deployment_reset_input(void);

/**
 * Remember a session clone for focused-first prewarming without retaining a raw handle.
 */
void keytao_set_focused_session(void *session);

/**
 * Rebuild sessions in focused-first order. Call on a background worker after reload.
 * Returns true only when at least one session was rebuilt.
 */
bool keytao_prewarm(void);

/**
 * Create a per-client input session. Returns null if keytao_init() has not
 * completed successfully. Destroy with keytao_destroy_session().
 *
 * The session belongs to the directories keytao_init() was last called with;
 * a later init for different ones retires it, and every call on it then
 * returns null or false.
 */
void *keytao_create_session(void);

/**
 * Destroy a session created by keytao_create_session(). Retired handles are
 * freed too; only their librime session is already gone.
 */
void keytao_destroy_session(void *session);

bool keytao_session_candidate_is_user_phrase(void *session, uint32_t index);

/**
 * Return whether a per-client session is in ASCII mode.
 */
bool keytao_session_get_ascii_mode(void *session);

/**
 * Logical English input (ASCII or the configured English dictionary schema).
 * The existing get_ascii_mode export continues to mean raw pass-through.
 */
bool keytao_session_get_english_mode(void *session);

char *keytao_session_set_english_mode_json(void *session, bool enabled);

/**
 * What the linked librime can do through its own entry points, as a mask of
 * the `KEYTAO_CAP_*` bits.
 *
 * A missing capability degrades to a synthesized key stroke whose meaning
 * depends on the schema — a select key, `-`/`=`, `Escape` — so a control that
 * needs one must be hidden rather than shown and quietly typing characters
 * into the composition.
 *
 * Answerable before keytao_init(): it only inspects the ABI.
 */
uint32_t keytao_engine_capabilities(void);

/**
 * The `KEYTAO_CAP_*` mask for the librime a session is composing against.
 *
 * Zero — every control disabled — for a null or retired handle, which is the
 * safe answer: a handle that cannot reach librime cannot select or page
 * either.
 */
uint32_t keytao_session_capabilities(void *session);

/**
 * The X11 keysym a soft keyboard key should send, or 0 when the text is not a
 * single typable character and has to be committed directly instead.
 *
 * Latin-1 maps onto itself and everything else uses X11's `0x01000000 | cp`
 * encoding, so `（` (U+FF08) never arrives as `XK_BackSpace`.
 */
uint32_t keytao_text_to_keysym(const char *text);

/**
 * Whether a keysym is `Return` or keypad `Return`.
 */
bool keytao_key_policy_is_enter(uint32_t keyval);

/**
 * Whether a key must be handed straight to the application instead of librime.
 *
 * `ascii_mode` deliberately plays no part: English mode still needs librime's
 * `ascii_composer` to see the key, and Control/Alt chords carry Rime's own
 * switcher hotkeys, so only window-system modifiers pass through early.
 */
bool keytao_key_policy_should_bypass(void *session, uint32_t keyval, uint32_t modifiers);

/**
 * Map a Unicode scalar offset into `text` to a UTF-16 code unit offset, the
 * unit IMKit, TSF and Android's InputConnection count in.
 */
uint32_t keytao_utf16_offset_from_chars(const char *text, uint32_t char_offset);

/**
 * What the frontend's candidate panel can actually render. A soft keyboard bar
 * and an IMKit panel disagree on this, and the theme layer needs to know which
 * one it is building a model for.
 */
void keytao_set_ui_capabilities(bool supports_custom_colors,
                                bool supports_vertical,
                                bool supports_hover,
                                bool supports_shadow,
                                bool supports_separator,
                                bool system_lookup_table_only);

/**
 * Configure optional default/user theme paths used by JSON state helpers.
 */
void keytao_set_theme_paths(const char *default_theme_path, const char *user_theme_path);

/**
 * Tell the JSON state helpers which color scheme the platform is showing, so
 * the IME process never probes the system itself. Pass null to go back to
 * keytao-theme's own detection.
 */
void keytao_set_system_color_scheme(const char *system_color_scheme);

/**
 * Resolve theme YAML from the optional default and user paths and return a
 * normalized JSON theme. The caller must free the string with
 * keytao_free_string().
 */
char *keytao_resolve_theme_json(const char *default_theme_path, const char *user_theme_path);

/**
 * Resolve theme YAML with a platform-provided system color scheme and return a
 * normalized JSON theme. The caller must free the string with
 * keytao_free_string().
 */
char *keytao_resolve_theme_json_with_system_scheme(const char *default_theme_path,
                                                   const char *user_theme_path,
                                                   const char *system_color_scheme);

/**
 * Resolve desktop renderer metadata and candidate metric scaling in Rust.
 */
char *keytao_resolve_renderer_theme_json_with_system_scheme(const char *default_theme_path,
                                                            const char *user_theme_path,
                                                            const char *system_color_scheme);

/**
 * Persist the mobile theme color scheme and optionally its accent. A null
 * accent preserves the key; an empty accent removes it.
 */
bool keytao_write_theme_ui(const char *path, const char *color_scheme, const char *accent_hex);

/**
 * Persist mobile appearance. A negative radius clears the override; non-finite
 * radii are rejected. Null colors preserve their keys; empty colors clear them.
 */
bool keytao_write_theme_appearance(const char *path,
                                   const char *color_scheme,
                                   const char *accent_hex,
                                   float corner_radius,
                                   const char *background_hex);

char *keytao_default_keyboard_yaml(void);

char *keytao_resolve_keyboard_json(const char *default_keyboard_path,
                                   const char *user_keyboard_path);

char *keytao_session_state_json(void *session);

char *keytao_session_process_key_json(void *session, uint32_t keyval, uint32_t modifiers);

char *keytao_session_process_key_with_panel_json(void *session,
                                                 uint32_t keyval,
                                                 uint32_t modifiers,
                                                 bool panel_visible,
                                                 bool vertical);

/**
 * Handle `Return` the way every frontend must: librime decides, and only if it
 * passes the key back is the raw input committed.
 */
char *keytao_session_process_enter_json(void *session);

char *keytao_session_select_candidate_json(void *session, uint32_t index);

char *keytao_session_delete_candidate_json(void *session, uint32_t index);

char *keytao_session_select_candidate_global_json(void *session, uint32_t index);

char *keytao_session_all_candidates_json(void *session, uint32_t limit);

char *keytao_session_list_schemas_json(void *session);

char *keytao_session_schema_switches_json(void *session);

char *keytao_session_current_schema_json(void *session);

char *keytao_session_select_schema_json(void *session, const char *schema_id);

bool keytao_session_get_option(void *session, const char *option_name);

char *keytao_session_set_option_json(void *session, const char *option_name, bool enabled);

char *keytao_session_change_page_json(void *session, bool backward);

char *keytao_session_reset_json(void *session);

char *keytao_session_commit_composition_json(void *session);

char *keytao_session_clear_composition_json(void *session);

/**
 * Declare what the current input context allows. Password and PIN fields pass
 * `composing = false`, which stops keys from reaching librime at all, so no
 * preedit appears and nothing is learned. Returns the state to apply.
 *
 * A null return means there was no state to hand back — the handle was
 * retired, or no engine was up — **not** that the policy was rejected. The
 * policy is recorded either way and applies to every later key, so a frontend
 * must never read null as "still composing" and fall back to sending the
 * password to librime.
 */
char *keytao_session_set_input_policy_json(void *session, bool composing, bool learning);

char *keytao_session_set_ascii_mode_json(void *session, bool enabled);

/**
 * Free a UTF-8 string returned by keytao-core-ffi.
 */
void keytao_free_string(char *ptr);

#endif  /* KEYTAO_CORE_H */
