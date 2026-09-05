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
 * Flat view of IME state returned to C callers.
 * All strings are null-terminated UTF-8. Free with keytao_free_state().
 */
typedef struct KeytaoState {
  char *preedit;
  /**
   * Caret position inside `preedit`, counted in Unicode scalars. Use
   * keytao_utf16_offset_from_chars() to map it to UTF-16 units.
   */
  uint32_t cursor;
  /**
   * Start of the selected (already converted) range inside `preedit`, in
   * Unicode scalars. Equal to `sel_end` when nothing is selected.
   */
  uint32_t sel_start;
  /**
   * End of the selected range inside `preedit`, in Unicode scalars.
   */
  uint32_t sel_end;
  char **candidate_texts;
  char **candidate_comments;
  uint32_t candidate_count;
  uint32_t highlighted_candidate_index;
  uint32_t page;
  bool is_last_page;
  char *committed;
  char *select_keys;
  bool ascii_mode;
  bool accepted;
} KeytaoState;

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
 * Path of the reload signal for the directory passed to keytao_init(), or null
 * before initialization. Free with keytao_free_string().
 */
char *keytao_reload_stamp_path(void);

/**
 * Current signature of the reload signal, or null when no stamp exists. The
 * format is keytao-core's and must not be reimplemented by frontends. Free
 * with keytao_free_string().
 */
char *keytao_reload_stamp_signature(void);

/**
 * Path of the reload signal inside `user_dir`, for frontends that watch the
 * file before keytao_init() has succeeded. Free with keytao_free_string().
 */
char *keytao_reload_stamp_path_at(const char *user_dir);

/**
 * Signature of the reload signal inside `user_dir`, or null when no deployment
 * has requested a reload yet. Same format keytao_reload_stamp_signature()
 * returns. Free with keytao_free_string().
 */
char *keytao_reload_stamp_signature_at(const char *user_dir);

/**
 * Whether the reload signal changed since the last call, which also marks it as
 * seen. Frontends that want to schedule the reload themselves use this;
 * keytao_reload_if_stamp_changed() does both in one step.
 */
bool keytao_reload_stamp_changed(void);

/**
 * Reload when the app requested one, returning true only if a reload actually
 * ran. Cheap enough for a timer, but not for the key path: it stats a file.
 */
bool keytao_reload_if_stamp_changed(void);

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

/**
 * Return current state for a per-client session.
 */
struct KeytaoState *keytao_session_state(void *session);

/**
 * Process a key event on a per-client session.
 */
struct KeytaoState *keytao_session_process_key(void *session, uint32_t keyval, uint32_t modifiers);

/**
 * Handle `Return` the way every frontend must: librime decides, and only if it
 * passes the key back is the raw input committed.
 */
struct KeytaoState *keytao_session_process_enter(void *session);

/**
 * Select a candidate in a per-client session.
 */
struct KeytaoState *keytao_session_select_candidate(void *session, uint32_t index);

/**
 * Move the highlight without committing, for hover and arrow-key navigation.
 */
struct KeytaoState *keytao_session_highlight_candidate(void *session, uint32_t index);

/**
 * Forget a learned phrase, the action behind "delete candidate" gestures.
 */
struct KeytaoState *keytao_session_delete_candidate(void *session, uint32_t index);

bool keytao_session_candidate_is_user_phrase(void *session, uint32_t index);

/**
 * Flip to the next/previous candidate page in a per-client session.
 */
struct KeytaoState *keytao_session_change_page(void *session, bool backward);

/**
 * Clear current composition in a per-client session.
 */
struct KeytaoState *keytao_session_reset(void *session);

/**
 * Commit what is being composed. The path to take when the input context ends
 * and the composition must not be lost.
 */
struct KeytaoState *keytao_session_commit_composition(void *session);

/**
 * Discard what is being composed. The path to take when the input context ends
 * and the composition must not reach the client.
 */
struct KeytaoState *keytao_session_clear_composition(void *session);

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
struct KeytaoState *keytao_session_set_input_policy(void *session, bool composing, bool learning);

/**
 * Whether the current input context still lets keys reach librime.
 */
bool keytao_session_input_policy_composing(void *session);

/**
 * Whether the current input context may contribute to user learning.
 */
bool keytao_session_input_policy_learning(void *session);

/**
 * Return whether a per-client session is in ASCII mode.
 */
bool keytao_session_get_ascii_mode(void *session);

/**
 * Set ASCII mode on a per-client session and return the updated state.
 */
struct KeytaoState *keytao_session_set_ascii_mode(void *session, bool enabled);

/**
 * What the linked librime can do through its own entry points, as a mask of
 * the `KEYTAO_CAP_*` bits.
 *
 * A missing capability degrades to a synthesized key stroke whose meaning
 * depends on the schema — a select key, `-`/`=`, `Escape` — so a control that
 * needs one must be hidden rather than shown and quietly typing characters
 * into the composition. The iOS build ships librime 1.8.5, where
 * `KEYTAO_CAP_NATIVE_PAGING` and `KEYTAO_CAP_CANDIDATE_HIGHLIGHT` are always
 * absent.
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
 * Same capabilities as keytao_engine_capabilities(), as a JSON object with the
 * camelCase keys the other JSON helpers use. Free with keytao_free_string().
 */
char *keytao_engine_capabilities_json(void);

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
 * Persist the mobile theme color scheme and optionally its accent. A null
 * accent preserves the key; an empty accent removes it.
 */
bool keytao_write_theme_ui(const char *path, const char *color_scheme, const char *accent_hex);

char *keytao_default_keyboard_yaml(void);

char *keytao_resolve_keyboard_json(const char *default_keyboard_path,
                                   const char *user_keyboard_path);

char *keytao_session_state_json(void *session);

char *keytao_session_process_key_json(void *session, uint32_t keyval, uint32_t modifiers);

char *keytao_session_process_enter_json(void *session);

char *keytao_session_select_candidate_json(void *session, uint32_t index);

char *keytao_session_highlight_candidate_json(void *session, uint32_t index);

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

char *keytao_session_set_input_policy_json(void *session, bool composing, bool learning);

char *keytao_session_set_ascii_mode_json(void *session, bool enabled);

/**
 * Free a UTF-8 string returned by keytao-core-ffi.
 */
void keytao_free_string(char *ptr);

/**
 * Process a key event. Returns heap-allocated KeytaoState; caller must free
 * with keytao_free_state(). Returns null if the runtime is not initialized.
 */
struct KeytaoState *keytao_process_key(uint32_t keyval, uint32_t modifiers);

/**
 * Select a candidate by 0-based index. Returns new state; caller must free.
 */
struct KeytaoState *keytao_select_candidate(uint32_t index);

/**
 * Flip to the next/previous candidate page. Returns new state; caller must free.
 */
struct KeytaoState *keytao_change_page(bool backward);

/**
 * Clear current composition. Returns new state; caller must free.
 */
struct KeytaoState *keytao_reset(void);

/**
 * Free a KeytaoState returned by any keytao_* function.
 */
void keytao_free_state(struct KeytaoState *ptr);

#endif  /* KEYTAO_CORE_H */
