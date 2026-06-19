#ifndef KEYTAO_CORE_H
#define KEYTAO_CORE_H

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * Flat view of IME state returned to C callers.
 * All strings are null-terminated UTF-8. Free with keytao_free_state().
 */
typedef struct KeytaoState {
  char *preedit;
  uint32_t cursor;
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
 */
bool keytao_init(const char *user_dir, const char *shared_dir);

bool keytao_is_initialized(void);

/**
 * Redeploy Rime data through the shared runtime. Existing sessions refresh
 * lazily on their next operation.
 */
bool keytao_reload(void);

/**
 * Create a per-client input session. Returns null if keytao_init() has not
 * completed successfully. Destroy with keytao_destroy_session().
 */
void *keytao_create_session(void);

/**
 * Destroy a session created by keytao_create_session().
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
 * Select a candidate in a per-client session.
 */
struct KeytaoState *keytao_session_select_candidate(void *session, uint32_t index);

/**
 * Flip to the next/previous candidate page in a per-client session.
 */
struct KeytaoState *keytao_session_change_page(void *session, bool backward);

/**
 * Clear current composition in a per-client session.
 */
struct KeytaoState *keytao_session_reset(void *session);

/**
 * Return whether a per-client session is in ASCII mode.
 */
bool keytao_session_get_ascii_mode(void *session);

/**
 * Set ASCII mode on a per-client session and return the updated state.
 */
struct KeytaoState *keytao_session_set_ascii_mode(void *session, bool enabled);

/**
 * Resolve theme YAML from the optional default and user paths and return a
 * normalized JSON theme. The caller must free the string with
 * keytao_free_string().
 */
char *keytao_resolve_theme_json(const char *default_theme_path, const char *user_theme_path);

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
 * Clear current composition (Escape). Returns new state; caller must free.
 */
struct KeytaoState *keytao_reset(void);

/**
 * Free a KeytaoState returned by any keytao_* function.
 */
void keytao_free_state(struct KeytaoState *ptr);

#endif  /* KEYTAO_CORE_H */
