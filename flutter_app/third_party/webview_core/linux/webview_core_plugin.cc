#include "include/webview_core/webview_core_plugin.h"

#include <flutter_linux/flutter_linux.h>
#include <gdk/gdkkeysyms.h>
#include <gtk/gtk.h>
#include <webkit2/webkit2.h>

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <map>
#include <memory>
#include <set>
#include <string>
#include <vector>

namespace {

bool IsWaylandDisplay() {
  GdkDisplay* display = gdk_display_get_default();
  if (display == nullptr) {
    return false;
  }
  const char* type_name = G_OBJECT_TYPE_NAME(display);
  return type_name != nullptr && g_str_has_prefix(type_name, "GdkWayland");
}

}  // namespace

// Native viewport container clipping a child WebKitWebView to the visible
// Flutter rect while preserving its unclipped layout size. On Wayland (where
// Flutter 3.50+ composites FlView into a wl_subsurface above GtkWindow's main
// surface), realizing this container as GDK_WINDOW_SUBSURFACE stacks the
// WebView surface above FlViewRendererSubsurface while keeping pass-through
// and clipping synchronized with the parent window.
struct _WebviewClipBox {
  GtkBin parent_instance;
  int child_offset_x;
  int child_offset_y;
  int child_width;
  int child_height;
  bool pass_through;
  bool is_subsurface;
  GtkWidget* watched_toplevel;
  gulong toplevel_map_id;
  gulong toplevel_destroy_id;
};

struct _WebviewClipBoxClass {
  GtkBinClass parent_class;
};

typedef struct _WebviewClipBox WebviewClipBox;
typedef struct _WebviewClipBoxClass WebviewClipBoxClass;

GType webview_clip_box_get_type(void);

G_DEFINE_TYPE(WebviewClipBox, webview_clip_box, GTK_TYPE_BIN)

#define WEBVIEW_CLIP_BOX(obj) \
  (G_TYPE_CHECK_INSTANCE_CAST((obj), webview_clip_box_get_type(), WebviewClipBox))
#define IS_WEBVIEW_CLIP_BOX(obj) \
  (G_TYPE_CHECK_INSTANCE_TYPE((obj), webview_clip_box_get_type()))

namespace {

void DisconnectWatchedToplevel(WebviewClipBox* self) {
  if (self == nullptr || self->watched_toplevel == nullptr) {
    return;
  }
  if (self->toplevel_map_id != 0) {
    g_signal_handler_disconnect(self->watched_toplevel, self->toplevel_map_id);
    self->toplevel_map_id = 0;
  }
  if (self->toplevel_destroy_id != 0) {
    g_signal_handler_disconnect(self->watched_toplevel,
                                self->toplevel_destroy_id);
    self->toplevel_destroy_id = 0;
  }
  self->watched_toplevel = nullptr;
}

void OnWatchedToplevelMapped(GtkWidget* /*toplevel*/, gpointer user_data) {
  WebviewClipBox* self = WEBVIEW_CLIP_BOX(user_data);
  DisconnectWatchedToplevel(self);
  GtkWidget* widget = GTK_WIDGET(self);
  if (gtk_widget_get_visible(widget) && !gtk_widget_get_mapped(widget)) {
    gtk_widget_map(widget);
  }
}

void OnWatchedToplevelDestroyed(GtkWidget* /*toplevel*/, gpointer user_data) {
  WebviewClipBox* self = WEBVIEW_CLIP_BOX(user_data);
  self->toplevel_map_id = 0;
  self->toplevel_destroy_id = 0;
  self->watched_toplevel = nullptr;
}

void ComputeClipWindowPosition(GtkWidget* widget,
                               const GtkAllocation* allocation,
                               bool is_subsurface,
                               int* out_x,
                               int* out_y) {
  int win_x = allocation->x;
  int win_y = allocation->y;
  if (is_subsurface) {
    GtkWidget* toplevel = gtk_widget_get_toplevel(widget);
    GdkWindow* top_win =
        (toplevel != nullptr && gtk_widget_is_toplevel(toplevel))
            ? gtk_widget_get_window(toplevel)
            : nullptr;
    GdkWindow* cur = gtk_widget_get_parent_window(widget);
    while (cur != nullptr && top_win != nullptr && cur != top_win) {
      int dx = 0;
      int dy = 0;
      gdk_window_get_position(cur, &dx, &dy);
      win_x += dx;
      win_y += dy;
      cur = gdk_window_get_parent(cur);
    }
  }
  *out_x = win_x;
  *out_y = win_y;
}

}  // namespace

static void webview_clip_box_get_preferred_width(GtkWidget* /*widget*/,
                                                 gint* minimum,
                                                 gint* natural) {
  if (minimum != nullptr) {
    *minimum = 0;
  }
  if (natural != nullptr) {
    *natural = 0;
  }
}

static void webview_clip_box_get_preferred_height(GtkWidget* /*widget*/,
                                                  gint* minimum,
                                                  gint* natural) {
  if (minimum != nullptr) {
    *minimum = 0;
  }
  if (natural != nullptr) {
    *natural = 0;
  }
}

static void webview_clip_box_realize(GtkWidget* widget) {
  WebviewClipBox* self = WEBVIEW_CLIP_BOX(widget);
  GtkAllocation allocation;
  gtk_widget_get_allocation(widget, &allocation);

  GtkWidget* toplevel = gtk_widget_get_toplevel(widget);
  GdkWindow* top_win =
      (toplevel != nullptr && gtk_widget_is_toplevel(toplevel))
          ? gtk_widget_get_window(toplevel)
          : nullptr;

  self->is_subsurface = IsWaylandDisplay() && top_win != nullptr;
  GdkWindow* parent_window =
      self->is_subsurface ? top_win : gtk_widget_get_parent_window(widget);

  int win_x = allocation.x;
  int win_y = allocation.y;
  ComputeClipWindowPosition(widget, &allocation, self->is_subsurface, &win_x,
                            &win_y);

  GdkWindowAttr attributes;
  std::memset(&attributes, 0, sizeof(attributes));
  attributes.window_type =
      self->is_subsurface ? GDK_WINDOW_SUBSURFACE : GDK_WINDOW_CHILD;
  attributes.x = win_x;
  attributes.y = win_y;
  attributes.width = std::max(1, allocation.width);
  attributes.height = std::max(1, allocation.height);
  attributes.wclass = GDK_INPUT_OUTPUT;
  attributes.visual = gtk_widget_get_visual(widget);
  attributes.event_mask =
      gtk_widget_get_events(widget) | GDK_EXPOSURE_MASK | GDK_STRUCTURE_MASK;

  const gint attributes_mask = GDK_WA_X | GDK_WA_Y | GDK_WA_VISUAL;
  GdkWindow* window =
      gdk_window_new(parent_window, &attributes, attributes_mask);
  if (self->is_subsurface && parent_window != nullptr) {
    gdk_window_set_transient_for(window, parent_window);
  }
  gtk_widget_set_window(widget, window);
  gtk_widget_register_window(widget, window);
  gtk_widget_set_realized(widget, TRUE);

  gdk_window_set_pass_through(window, self->pass_through ? TRUE : FALSE);

  GtkWidget* child = gtk_bin_get_child(GTK_BIN(widget));
  if (child != nullptr) {
    gtk_widget_set_parent_window(child, window);
  }
}

static void webview_clip_box_unrealize(GtkWidget* widget) {
  WebviewClipBox* self = WEBVIEW_CLIP_BOX(widget);
  DisconnectWatchedToplevel(self);
  GTK_WIDGET_CLASS(webview_clip_box_parent_class)->unrealize(widget);
}

static void webview_clip_box_map(GtkWidget* widget) {
  WebviewClipBox* self = WEBVIEW_CLIP_BOX(widget);
  if (self->is_subsurface) {
    GtkWidget* toplevel = gtk_widget_get_toplevel(widget);
    if (toplevel != nullptr && gtk_widget_is_toplevel(toplevel) &&
        !gtk_widget_get_mapped(toplevel)) {
      if (self->watched_toplevel != toplevel) {
        DisconnectWatchedToplevel(self);
        self->watched_toplevel = toplevel;
        self->toplevel_map_id = g_signal_connect(
            toplevel, "map", G_CALLBACK(OnWatchedToplevelMapped), self);
        self->toplevel_destroy_id = g_signal_connect(
            toplevel, "destroy", G_CALLBACK(OnWatchedToplevelDestroyed), self);
      }
      return;
    }
  }
  DisconnectWatchedToplevel(self);
  GTK_WIDGET_CLASS(webview_clip_box_parent_class)->map(widget);
  GdkWindow* window = gtk_widget_get_window(widget);
  if (window != nullptr) {
    gdk_window_set_pass_through(window, self->pass_through ? TRUE : FALSE);
  }
}

static void webview_clip_box_unmap(GtkWidget* widget) {
  WebviewClipBox* self = WEBVIEW_CLIP_BOX(widget);
  DisconnectWatchedToplevel(self);
  GTK_WIDGET_CLASS(webview_clip_box_parent_class)->unmap(widget);
}

static void webview_clip_box_size_allocate(GtkWidget* widget,
                                           GtkAllocation* allocation) {
  WebviewClipBox* self = WEBVIEW_CLIP_BOX(widget);
  gtk_widget_set_allocation(widget, allocation);

  if (gtk_widget_get_realized(widget)) {
    GdkWindow* window = gtk_widget_get_window(widget);
    if (window != nullptr) {
      int win_x = allocation->x;
      int win_y = allocation->y;
      ComputeClipWindowPosition(widget, allocation, self->is_subsurface, &win_x,
                                &win_y);
      gdk_window_move_resize(window, win_x, win_y,
                             std::max(1, allocation->width),
                             std::max(1, allocation->height));
    }
  }

  GtkWidget* child = gtk_bin_get_child(GTK_BIN(widget));
  if (child != nullptr && gtk_widget_get_visible(child)) {
    GtkAllocation child_alloc;
    child_alloc.x = self->child_offset_x;
    child_alloc.y = self->child_offset_y;
    child_alloc.width = std::max(1, self->child_width);
    child_alloc.height = std::max(1, self->child_height);
    gtk_widget_size_allocate(child, &child_alloc);
  }
}

static void webview_clip_box_add(GtkContainer* container, GtkWidget* child) {
  WebviewClipBox* self = WEBVIEW_CLIP_BOX(container);
  if (gtk_widget_get_realized(GTK_WIDGET(self))) {
    GdkWindow* window = gtk_widget_get_window(GTK_WIDGET(self));
    if (window != nullptr) {
      gtk_widget_set_parent_window(child, window);
    }
  }
  GTK_CONTAINER_CLASS(webview_clip_box_parent_class)->add(container, child);
}

static void webview_clip_box_class_init(WebviewClipBoxClass* klass) {
  GtkWidgetClass* widget_class = GTK_WIDGET_CLASS(klass);
  widget_class->get_preferred_width = webview_clip_box_get_preferred_width;
  widget_class->get_preferred_height = webview_clip_box_get_preferred_height;
  widget_class->realize = webview_clip_box_realize;
  widget_class->unrealize = webview_clip_box_unrealize;
  widget_class->map = webview_clip_box_map;
  widget_class->unmap = webview_clip_box_unmap;
  widget_class->size_allocate = webview_clip_box_size_allocate;

  GtkContainerClass* container_class = GTK_CONTAINER_CLASS(klass);
  container_class->add = webview_clip_box_add;
}

static void webview_clip_box_init(WebviewClipBox* self) {
  gtk_widget_set_has_window(GTK_WIDGET(self), TRUE);
  gtk_widget_set_redraw_on_allocate(GTK_WIDGET(self), FALSE);
  self->child_offset_x = 0;
  self->child_offset_y = 0;
  self->child_width = 1;
  self->child_height = 1;
  self->pass_through = true;
  self->is_subsurface = false;
  self->watched_toplevel = nullptr;
  self->toplevel_map_id = 0;
  self->toplevel_destroy_id = 0;
}

namespace {

GtkWidget* CreateWebviewClipBox(GtkWidget* child) {
  GtkWidget* box =
      GTK_WIDGET(g_object_new(webview_clip_box_get_type(), nullptr));
  if (child != nullptr) {
    gtk_container_add(GTK_CONTAINER(box), child);
  }
  return box;
}

void UpdateWebviewClipBoxChildLayout(GtkWidget* clip_widget,
                                     int offset_x,
                                     int offset_y,
                                     int full_width,
                                     int full_height) {
  if (!IS_WEBVIEW_CLIP_BOX(clip_widget)) {
    return;
  }
  WebviewClipBox* self = WEBVIEW_CLIP_BOX(clip_widget);
  const int safe_w = std::max(1, full_width);
  const int safe_h = std::max(1, full_height);
  if (self->child_offset_x == offset_x && self->child_offset_y == offset_y &&
      self->child_width == safe_w && self->child_height == safe_h) {
    return;
  }
  self->child_offset_x = offset_x;
  self->child_offset_y = offset_y;
  self->child_width = safe_w;
  self->child_height = safe_h;
  gtk_widget_queue_allocate(clip_widget);
}

void SetWebviewClipBoxPassThrough(GtkOverlay* overlay,
                                  GtkWidget* clip_widget,
                                  bool pass_through) {
  if (overlay != nullptr && clip_widget != nullptr &&
      gtk_widget_get_parent(clip_widget) == GTK_WIDGET(overlay)) {
    gtk_overlay_set_overlay_pass_through(overlay, clip_widget,
                                         pass_through ? TRUE : FALSE);
  }
  if (IS_WEBVIEW_CLIP_BOX(clip_widget)) {
    WebviewClipBox* self = WEBVIEW_CLIP_BOX(clip_widget);
    self->pass_through = pass_through;
    if (gtk_widget_get_realized(clip_widget)) {
      GdkWindow* window = gtk_widget_get_window(clip_widget);
      if (window != nullptr) {
        gdk_window_set_pass_through(window, pass_through ? TRUE : FALSE);
      }
    }
  }
}

void SyncWebviewClipBoxWindowGeometry(GtkWidget* clip_widget) {
  if (!IS_WEBVIEW_CLIP_BOX(clip_widget) ||
      !gtk_widget_get_realized(clip_widget)) {
    return;
  }
  WebviewClipBox* self = WEBVIEW_CLIP_BOX(clip_widget);
  GdkWindow* window = gtk_widget_get_window(clip_widget);
  if (window == nullptr) {
    return;
  }
  GtkAllocation allocation;
  gtk_widget_get_allocation(clip_widget, &allocation);
  int win_x = allocation.x;
  int win_y = allocation.y;
  ComputeClipWindowPosition(clip_widget, &allocation, self->is_subsurface,
                            &win_x, &win_y);
  gdk_window_move_resize(window, win_x, win_y, std::max(1, allocation.width),
                         std::max(1, allocation.height));
}

void DismissTransientWebViewPopupsAndGrabs(GtkWidget* clip_container,
                                           GtkWidget* webview) {
  if (webview != nullptr) {
    GList* attached = gtk_menu_get_for_attach_widget(webview);
    std::vector<GtkMenu*> menus;
    for (GList* l = attached; l != nullptr; l = l->next) {
      if (GTK_IS_MENU(l->data)) {
        menus.push_back(GTK_MENU(l->data));
      }
    }
    for (size_t i = 0; i < menus.size(); ++i) {
      if (gtk_widget_get_visible(GTK_WIDGET(menus[i]))) {
        gtk_menu_popdown(menus[i]);
      }
    }
  }
  GtkWidget* grabbed = gtk_grab_get_current();
  if (grabbed != nullptr) {
    const bool in_clip =
        clip_container != nullptr &&
        (grabbed == clip_container ||
         gtk_widget_is_ancestor(grabbed, clip_container));
    const bool in_webview =
        webview != nullptr &&
        (grabbed == webview || gtk_widget_is_ancestor(grabbed, webview));
    if (in_clip || in_webview) {
      gtk_grab_remove(grabbed);
    }
  }
}

std::string EscapeJsonString(const std::string& input) {
  std::string out;
  out.reserve(input.size() + 2);
  out.push_back('"');
  for (size_t i = 0; i < input.size(); ++i) {
    const unsigned char c = static_cast<unsigned char>(input[i]);
    switch (c) {
      case '"':
        out += "\\\"";
        break;
      case '\\':
        out += "\\\\";
        break;
      case '\n':
        out += "\\n";
        break;
      case '\r':
        out += "\\r";
        break;
      case '\t':
        out += "\\t";
        break;
      default:
        if (c < 0x20) {
          char buf[8];
          snprintf(buf, sizeof(buf), "\\u%04x", static_cast<unsigned int>(c));
          out += buf;
        } else {
          out.push_back(static_cast<char>(c));
        }
        break;
    }
  }
  out.push_back('"');
  return out;
}

FlValue* LookupMapValue(FlValue* map, const char* key) {
  if (map == nullptr || fl_value_get_type(map) != FL_VALUE_TYPE_MAP) {
    return nullptr;
  }
  return fl_value_lookup_string(map, key);
}

bool GetIntArg(FlValue* map, const char* key, int64_t* out) {
  FlValue* val = LookupMapValue(map, key);
  if (val == nullptr || fl_value_get_type(val) != FL_VALUE_TYPE_INT) {
    return false;
  }
  *out = fl_value_get_int(val);
  return true;
}

bool GetBoolArg(FlValue* map, const char* key, bool default_val) {
  FlValue* val = LookupMapValue(map, key);
  if (val == nullptr || fl_value_get_type(val) != FL_VALUE_TYPE_BOOL) {
    return default_val;
  }
  return fl_value_get_bool(val) != FALSE;
}

std::string GetStringArg(FlValue* map, const char* key, bool* found = nullptr) {
  FlValue* val = LookupMapValue(map, key);
  if (val == nullptr || fl_value_get_type(val) != FL_VALUE_TYPE_STRING) {
    if (found != nullptr) {
      *found = false;
    }
    return std::string();
  }
  if (found != nullptr) {
    *found = true;
  }
  const char* raw = fl_value_get_string(val);
  return raw != nullptr ? std::string(raw) : std::string();
}

FlValue* JscValueToFlValue(JSCValue* value) {
  if (value == nullptr || jsc_value_is_null(value) ||
      jsc_value_is_undefined(value)) {
    return fl_value_new_null();
  }
  if (jsc_value_is_boolean(value)) {
    return fl_value_new_bool(jsc_value_to_boolean(value));
  }
  if (jsc_value_is_number(value)) {
    const double d = jsc_value_to_double(value);
    const double rounded = std::floor(d);
    if (std::isfinite(d) && d == rounded && d >= -9007199254740991.0 &&
        d <= 9007199254740991.0) {
      return fl_value_new_int(static_cast<int64_t>(d));
    }
    return fl_value_new_float(d);
  }
  if (jsc_value_is_string(value)) {
    gchar* str = jsc_value_to_string(value);
    FlValue* res = fl_value_new_string(str != nullptr ? str : "");
    g_free(str);
    return res;
  }
  gchar* json = jsc_value_to_json(value, 0);
  if (json != nullptr) {
    FlValue* res = fl_value_new_string(json);
    g_free(json);
    return res;
  }
  gchar* fallback = jsc_value_to_string(value);
  FlValue* res = fl_value_new_string(fallback != nullptr ? fallback : "");
  g_free(fallback);
  return res;
}

struct LinuxWebViewInstance;

struct PluginState {
  FlPluginRegistrar* registrar = nullptr;
  FlMethodChannel* channel = nullptr;
  GtkOverlay* overlay = nullptr;
  GtkWidget* flutter_event_box = nullptr;
  GtkWidget* tracked_fl_view = nullptr;
  gulong child_pos_handler_id = 0;
  gulong overlay_size_alloc_id = 0;
  gulong overlay_destroy_id = 0;
  gulong event_box_press_id = 0;
  gulong event_box_destroy_id = 0;
  gulong fl_view_press_id = 0;
  gulong fl_view_destroy_id = 0;
  std::map<int64_t, std::unique_ptr<LinuxWebViewInstance>> instances;
};

void RestoreFlViewFocusIfNeeded(PluginState* state, GtkWidget* inst_root) {
  if (state == nullptr || state->registrar == nullptr) {
    return;
  }
  FlView* fl_view = fl_plugin_registrar_get_view(state->registrar);
  if (fl_view == nullptr) {
    return;
  }
  GtkWidget* view_widget = GTK_WIDGET(fl_view);
  if (!gtk_widget_get_visible(view_widget) ||
      !gtk_widget_get_can_focus(view_widget)) {
    return;
  }
  GtkWidget* toplevel = gtk_widget_get_toplevel(view_widget);
  if (!GTK_IS_WINDOW(toplevel)) {
    return;
  }
  GtkWidget* focused = gtk_window_get_focus(GTK_WINDOW(toplevel));
  if (focused == nullptr ||
      (inst_root != nullptr &&
       (focused == inst_root || gtk_widget_is_ancestor(focused, inst_root)))) {
    gtk_widget_grab_focus(view_widget);
  }
}

struct ChannelCallbackData {
  PluginState* state = nullptr;
  int64_t controller_id = 0;
  std::string channel_name;
};

struct PendingPolicyData {
  WebKitPolicyDecision* decision = nullptr;
};

struct PendingJsCallData {
  FlMethodCall* call = nullptr;
  bool return_result = false;
  bool is_scroll_pos = false;
};

struct LinuxWebViewInstance {
  PluginState* state = nullptr;
  int64_t controller_id = 0;
  GtkWidget* clip_container = nullptr;
  GtkWidget* webview = nullptr;
  WebKitUserContentManager* ucm = nullptr;
  bool visible = false;
  bool pass_through = true;
  int x = 0;
  int y = 0;
  int width = 0;
  int height = 0;
  int full_x = 0;
  int full_y = 0;
  int full_width = 0;
  int full_height = 0;
  bool has_navigation_decision = false;
  bool zoom_enabled = true;
  bool loaded_from_html_string = false;
  bool in_inline_html_load = false;
  bool has_last_base_url = false;
  std::string last_html;
  std::string last_base_url;
  std::string default_user_agent;
  std::map<std::string, gulong> channel_signal_ids;
  std::vector<std::unique_ptr<ChannelCallbackData>> channel_datas;
  gulong load_changed_id = 0;
  gulong load_failed_id = 0;
  gulong notify_uri_id = 0;
  gulong notify_progress_id = 0;
  gulong decide_policy_id = 0;
  gulong create_id = 0;
  gulong key_press_id = 0;
  gulong webview_destroy_id = 0;
  gulong clip_destroy_id = 0;
  bool destroyed = false;

  ~LinuxWebViewInstance() { Destroy(); }

  void Destroy() {
    if (destroyed) {
      return;
    }
    destroyed = true;
    DismissTransientWebViewPopupsAndGrabs(clip_container, webview);
    RestoreFlViewFocusIfNeeded(state, clip_container != nullptr ? clip_container
                                                                : webview);
    if (ucm != nullptr) {
      for (std::map<std::string, gulong>::iterator it =
               channel_signal_ids.begin();
           it != channel_signal_ids.end(); ++it) {
        if (it->second != 0) {
          g_signal_handler_disconnect(ucm, it->second);
        }
        webkit_user_content_manager_unregister_script_message_handler(
            ucm, it->first.c_str());
      }
      channel_signal_ids.clear();
      webkit_user_content_manager_remove_all_scripts(ucm);
      g_clear_object(&ucm);
    }
    if (webview != nullptr) {
      if (load_changed_id != 0) {
        g_signal_handler_disconnect(webview, load_changed_id);
        load_changed_id = 0;
      }
      if (load_failed_id != 0) {
        g_signal_handler_disconnect(webview, load_failed_id);
        load_failed_id = 0;
      }
      if (notify_uri_id != 0) {
        g_signal_handler_disconnect(webview, notify_uri_id);
        notify_uri_id = 0;
      }
      if (notify_progress_id != 0) {
        g_signal_handler_disconnect(webview, notify_progress_id);
        notify_progress_id = 0;
      }
      if (decide_policy_id != 0) {
        g_signal_handler_disconnect(webview, decide_policy_id);
        decide_policy_id = 0;
      }
      if (create_id != 0) {
        g_signal_handler_disconnect(webview, create_id);
        create_id = 0;
      }
      if (key_press_id != 0) {
        g_signal_handler_disconnect(webview, key_press_id);
        key_press_id = 0;
      }
      if (webview_destroy_id != 0) {
        g_signal_handler_disconnect(webview, webview_destroy_id);
        webview_destroy_id = 0;
      }
      webkit_web_view_stop_loading(WEBKIT_WEB_VIEW(webview));
      gtk_widget_hide(webview);
      if (clip_container == nullptr) {
        GtkWidget* parent = gtk_widget_get_parent(webview);
        if (parent != nullptr) {
          gtk_container_remove(GTK_CONTAINER(parent), webview);
        } else {
          gtk_widget_destroy(webview);
        }
      }
      webview = nullptr;
    }
    if (clip_container != nullptr) {
      if (clip_destroy_id != 0) {
        g_signal_handler_disconnect(clip_container, clip_destroy_id);
        clip_destroy_id = 0;
      }
      gtk_widget_hide(clip_container);
      GtkWidget* parent = gtk_widget_get_parent(clip_container);
      if (parent != nullptr) {
        gtk_container_remove(GTK_CONTAINER(parent), clip_container);
      } else {
        gtk_widget_destroy(clip_container);
      }
      clip_container = nullptr;
    }
    last_html.clear();
    last_base_url.clear();
    channel_datas.clear();
  }
};

void OnWebViewDestroyed(GtkWidget* /*widget*/, gpointer user_data) {
  LinuxWebViewInstance* inst = static_cast<LinuxWebViewInstance*>(user_data);
  if (inst == nullptr) {
    return;
  }
  inst->load_changed_id = 0;
  inst->load_failed_id = 0;
  inst->notify_uri_id = 0;
  inst->notify_progress_id = 0;
  inst->decide_policy_id = 0;
  inst->create_id = 0;
  inst->key_press_id = 0;
  inst->webview_destroy_id = 0;
  inst->webview = nullptr;
}

void OnClipContainerDestroyed(GtkWidget* /*widget*/, gpointer user_data) {
  LinuxWebViewInstance* inst = static_cast<LinuxWebViewInstance*>(user_data);
  if (inst == nullptr) {
    return;
  }
  inst->clip_destroy_id = 0;
  inst->clip_container = nullptr;
}

void OnOverlayDestroyed(GtkWidget* /*widget*/, gpointer user_data) {
  PluginState* state = static_cast<PluginState*>(user_data);
  if (state == nullptr) {
    return;
  }
  state->child_pos_handler_id = 0;
  state->overlay_size_alloc_id = 0;
  state->overlay_destroy_id = 0;
  state->overlay = nullptr;
}

void OnFlutterEventBoxDestroyed(GtkWidget* /*widget*/, gpointer user_data) {
  PluginState* state = static_cast<PluginState*>(user_data);
  if (state == nullptr) {
    return;
  }
  state->event_box_press_id = 0;
  state->event_box_destroy_id = 0;
  state->flutter_event_box = nullptr;
}

void OnTrackedFlViewDestroyed(GtkWidget* /*widget*/, gpointer user_data) {
  PluginState* state = static_cast<PluginState*>(user_data);
  if (state == nullptr) {
    return;
  }
  state->fl_view_press_id = 0;
  state->fl_view_destroy_id = 0;
  state->tracked_fl_view = nullptr;
}

// FlView registers its own "button-press-event" handler on the internal
// GtkEventBox during fl_view_init and returns TRUE (GDK_EVENT_STOP) without
// calling gtk_widget_grab_focus(fl_view), so a later "button-press-event"
// listener would never be reached. Connecting to "event" runs prior to
// "button-press-event", restores GTK focus to FlView on mouse/touch press, and
// returns FALSE so FlView still processes the pointer event normally.
gboolean OnFlutterSurfaceEvent(GtkWidget* /*widget*/,
                               GdkEvent* event,
                               gpointer user_data) {
  if (event == nullptr) {
    return FALSE;
  }
  const GdkEventType type = event->type;
  if (type != GDK_BUTTON_PRESS && type != GDK_2BUTTON_PRESS &&
      type != GDK_3BUTTON_PRESS && type != GDK_TOUCH_BEGIN) {
    return FALSE;
  }
  PluginState* state = static_cast<PluginState*>(user_data);
  if (state != nullptr && state->registrar != nullptr) {
    FlView* fl_view = fl_plugin_registrar_get_view(state->registrar);
    if (fl_view != nullptr) {
      GtkWidget* view_widget = GTK_WIDGET(fl_view);
      if (gtk_widget_get_can_focus(view_widget) &&
          (!gtk_widget_is_focus(view_widget) ||
           !gtk_widget_has_focus(view_widget))) {
        gtk_widget_grab_focus(view_widget);
      }
    }
  }
  return FALSE;
}

GtkWidget* FindFirstEventBox(GtkWidget* root) {
  if (root == nullptr) {
    return nullptr;
  }
  if (GTK_IS_EVENT_BOX(root)) {
    return root;
  }
  if (!GTK_IS_CONTAINER(root)) {
    return nullptr;
  }
  GList* children = gtk_container_get_children(GTK_CONTAINER(root));
  GtkWidget* found = nullptr;
  for (GList* l = children; l != nullptr && found == nullptr; l = l->next) {
    found = FindFirstEventBox(GTK_WIDGET(l->data));
  }
  g_list_free(children);
  return found;
}

void EnsureFlutterFocusHooks(PluginState* state, GtkWidget* view_widget) {
  if (state == nullptr || view_widget == nullptr) {
    return;
  }
  if (state->tracked_fl_view != view_widget) {
    if (state->tracked_fl_view != nullptr) {
      if (state->fl_view_press_id != 0) {
        g_signal_handler_disconnect(state->tracked_fl_view,
                                    state->fl_view_press_id);
      }
      if (state->fl_view_destroy_id != 0) {
        g_signal_handler_disconnect(state->tracked_fl_view,
                                    state->fl_view_destroy_id);
      }
    }
    state->tracked_fl_view = view_widget;
    state->fl_view_press_id = g_signal_connect(
        view_widget, "event", G_CALLBACK(OnFlutterSurfaceEvent), state);
    state->fl_view_destroy_id = g_signal_connect(
        view_widget, "destroy", G_CALLBACK(OnTrackedFlViewDestroyed), state);
  }

  GtkWidget* event_box = FindFirstEventBox(view_widget);
  if (event_box != nullptr && state->flutter_event_box != event_box) {
    if (state->flutter_event_box != nullptr) {
      if (state->event_box_press_id != 0) {
        g_signal_handler_disconnect(state->flutter_event_box,
                                    state->event_box_press_id);
      }
      if (state->event_box_destroy_id != 0) {
        g_signal_handler_disconnect(state->flutter_event_box,
                                    state->event_box_destroy_id);
      }
    }
    state->flutter_event_box = event_box;
    state->event_box_press_id = g_signal_connect(
        event_box, "event", G_CALLBACK(OnFlutterSurfaceEvent), state);
    state->event_box_destroy_id = g_signal_connect(
        event_box, "destroy", G_CALLBACK(OnFlutterEventBoxDestroyed), state);
  }
}

void SendChannelEvent(PluginState* state, const char* method, FlValue* args) {
  if (state == nullptr || state->channel == nullptr) {
    if (args != nullptr) {
      fl_value_unref(args);
    }
    return;
  }
  fl_method_channel_invoke_method(state->channel, method, args, nullptr,
                                  nullptr, nullptr);
  if (args != nullptr) {
    fl_value_unref(args);
  }
}

gboolean OnOverlayGetChildPosition(GtkOverlay* /*overlay*/,
                                   GtkWidget* widget,
                                   GdkRectangle* allocation,
                                   gpointer user_data) {
  PluginState* state = static_cast<PluginState*>(user_data);
  if (state == nullptr) {
    return FALSE;
  }
  for (std::map<int64_t, std::unique_ptr<LinuxWebViewInstance>>::iterator it =
           state->instances.begin();
       it != state->instances.end(); ++it) {
    LinuxWebViewInstance* inst = it->second.get();
    if (inst != nullptr && inst->clip_container == widget) {
      allocation->x = inst->x;
      allocation->y = inst->y;
      allocation->width = inst->width > 0 ? inst->width : 1;
      allocation->height = inst->height > 0 ? inst->height : 1;
      return TRUE;
    }
  }
  return FALSE;
}

void OnOverlaySizeAllocate(GtkWidget* /*overlay*/,
                           GtkAllocation* /*allocation*/,
                           gpointer user_data) {
  PluginState* state = static_cast<PluginState*>(user_data);
  if (state == nullptr) {
    return;
  }
  for (std::map<int64_t, std::unique_ptr<LinuxWebViewInstance>>::iterator it =
           state->instances.begin();
       it != state->instances.end(); ++it) {
    LinuxWebViewInstance* inst = it->second.get();
    if (inst != nullptr && inst->visible && inst->clip_container != nullptr) {
      SyncWebviewClipBoxWindowGeometry(inst->clip_container);
    }
  }
}

GtkOverlay* EnsureOverlay(PluginState* state) {
  if (state == nullptr || state->registrar == nullptr) {
    return nullptr;
  }
  FlView* fl_view = fl_plugin_registrar_get_view(state->registrar);
  if (fl_view == nullptr) {
    return nullptr;
  }
  GtkWidget* view_widget = GTK_WIDGET(fl_view);
  EnsureFlutterFocusHooks(state, view_widget);

  if (state->overlay != nullptr) {
    return state->overlay;
  }

  const bool view_had_focus = gtk_widget_has_focus(view_widget) != FALSE;

  // In Flutter 3.x / 3.50+, FlView inherits from GtkBox and is realized prior
  // to plugin registration. Removing FlView from its parent GtkWindow triggers
  // fl_view_unrealize, which tears down the engine view and stalls the Dart UI
  // isolate. Instead, pack GtkOverlay inside FlView and reparent FlView's
  // internal GtkEventBox into the already-realized GtkOverlay so neither FlView
  // nor its renderer surface is ever unrealized.
  if (GTK_IS_BOX(view_widget)) {
    GList* children = gtk_container_get_children(GTK_CONTAINER(view_widget));
    GtkWidget* first_child =
        children != nullptr ? GTK_WIDGET(children->data) : nullptr;
    g_list_free(children);

    if (first_child != nullptr && GTK_IS_OVERLAY(first_child)) {
      state->overlay = GTK_OVERLAY(first_child);
    } else if (first_child != nullptr) {
      gboolean expand = TRUE;
      gboolean fill = TRUE;
      guint padding = 0;
      GtkPackType pack_type = GTK_PACK_START;
      gtk_box_query_child_packing(GTK_BOX(view_widget), first_child, &expand,
                                  &fill, &padding, &pack_type);

      GtkWidget* overlay_widget = gtk_overlay_new();
      gtk_widget_set_hexpand(overlay_widget, TRUE);
      gtk_widget_set_vexpand(overlay_widget, TRUE);
      gtk_box_pack_start(GTK_BOX(view_widget), overlay_widget, expand, fill,
                         padding);
      gtk_box_set_child_packing(GTK_BOX(view_widget), overlay_widget, expand,
                                fill, padding, pack_type);
      gtk_box_reorder_child(GTK_BOX(view_widget), overlay_widget, 0);
      gtk_widget_show(overlay_widget);

      gtk_widget_set_hexpand(first_child, TRUE);
      gtk_widget_set_vexpand(first_child, TRUE);
      G_GNUC_BEGIN_IGNORE_DEPRECATIONS
      gtk_widget_reparent(first_child, overlay_widget);
      G_GNUC_END_IGNORE_DEPRECATIONS

      state->overlay = GTK_OVERLAY(overlay_widget);
    }
  }

  if (state->overlay == nullptr) {
    GtkWidget* parent = gtk_widget_get_parent(view_widget);
    if (parent == nullptr) {
      return nullptr;
    }
    if (GTK_IS_OVERLAY(parent)) {
      state->overlay = GTK_OVERLAY(parent);
    } else {
      gboolean expand = TRUE;
      gboolean fill = TRUE;
      guint padding = 0;
      GtkPackType pack_type = GTK_PACK_START;
      gint position = 0;
      const bool parent_is_box = GTK_IS_BOX(parent) != FALSE;
      if (parent_is_box) {
        gtk_box_query_child_packing(GTK_BOX(parent), view_widget, &expand,
                                    &fill, &padding, &pack_type);
        GList* siblings = gtk_container_get_children(GTK_CONTAINER(parent));
        position = g_list_index(siblings, view_widget);
        g_list_free(siblings);
      }

      GtkWidget* overlay_widget = gtk_overlay_new();
      gtk_widget_set_hexpand(overlay_widget, TRUE);
      gtk_widget_set_vexpand(overlay_widget, TRUE);
      gtk_widget_show(overlay_widget);
      g_object_ref(view_widget);
      gtk_container_remove(GTK_CONTAINER(parent), view_widget);
      gtk_container_add(GTK_CONTAINER(overlay_widget), view_widget);
      gtk_container_add(GTK_CONTAINER(parent), overlay_widget);
      if (parent_is_box) {
        gtk_box_set_child_packing(GTK_BOX(parent), overlay_widget, expand, fill,
                                  padding, pack_type);
        if (position >= 0) {
          gtk_box_reorder_child(GTK_BOX(parent), overlay_widget, position);
        }
      }
      g_object_unref(view_widget);
      state->overlay = GTK_OVERLAY(overlay_widget);
    }
  }

  if (view_had_focus && !gtk_widget_has_focus(view_widget)) {
    gtk_widget_grab_focus(view_widget);
  }

  state->child_pos_handler_id = g_signal_connect(
      state->overlay, "get-child-position",
      G_CALLBACK(OnOverlayGetChildPosition), state);
  state->overlay_size_alloc_id = g_signal_connect(
      state->overlay, "size-allocate",
      G_CALLBACK(OnOverlaySizeAllocate), state);
  state->overlay_destroy_id = g_signal_connect(
      state->overlay, "destroy", G_CALLBACK(OnOverlayDestroyed), state);
  return state->overlay;
}

void LoadHtmlOnWebView(LinuxWebViewInstance* inst,
                       const std::string& html,
                       bool has_base,
                       const std::string& base_url) {
  if (inst == nullptr || inst->webview == nullptr) {
    return;
  }
  inst->loaded_from_html_string = true;
  inst->in_inline_html_load = true;
  inst->last_html = html;
  inst->has_last_base_url = has_base && !base_url.empty();
  inst->last_base_url = inst->has_last_base_url ? base_url : std::string();
  webkit_web_view_load_html(
      WEBKIT_WEB_VIEW(inst->webview), inst->last_html.c_str(),
      inst->has_last_base_url ? inst->last_base_url.c_str() : nullptr);
}

void RebuildUserScripts(LinuxWebViewInstance* inst) {
  if (inst == nullptr || inst->ucm == nullptr) {
    return;
  }
  webkit_user_content_manager_remove_all_scripts(inst->ucm);
  if (!inst->zoom_enabled) {
    const char* no_zoom_src =
        "window.addEventListener('wheel',function(e){"
        "if(e.ctrlKey){e.preventDefault();}},{passive:false});"
        "window.addEventListener('keydown',function(e){"
        "if((e.ctrlKey||e.metaKey)&&(e.key==='+'||e.key==='-'||e.key==='='||"
        "e.key==='0')){e.preventDefault();}},false);";
    WebKitUserScript* nz_script = webkit_user_script_new(
        no_zoom_src, WEBKIT_USER_CONTENT_INJECT_ALL_FRAMES,
        WEBKIT_USER_SCRIPT_INJECT_AT_DOCUMENT_START, nullptr, nullptr);
    webkit_user_content_manager_add_script(inst->ucm, nz_script);
    webkit_user_script_unref(nz_script);
  }
  for (std::map<std::string, gulong>::iterator it =
           inst->channel_signal_ids.begin();
       it != inst->channel_signal_ids.end(); ++it) {
    const std::string quoted = EscapeJsonString(it->first);
    const std::string script_src =
        "window[" + quoted + "]={postMessage:function(m){"
        "window.webkit.messageHandlers[" +
        quoted + "].postMessage(String(m));}};";
    WebKitUserScript* script = webkit_user_script_new(
        script_src.c_str(), WEBKIT_USER_CONTENT_INJECT_ALL_FRAMES,
        WEBKIT_USER_SCRIPT_INJECT_AT_DOCUMENT_START, nullptr, nullptr);
    webkit_user_content_manager_add_script(inst->ucm, script);
    webkit_user_script_unref(script);
  }
}

// Note: In webkit2gtk-4.1 (GTK3 ABI, !ENABLE(2022_GLIB_API)), the
// "script-message-received" signal always passes WebKitJavascriptResult* and
// register/unregister_script_message_handler always take 2 arguments across
// all 2.36..2.48+ releases (the 3-arg signature is exclusive to webkitgtk-6.0).
void OnScriptMessageReceived(WebKitUserContentManager* /*manager*/,
                             WebKitJavascriptResult* js_result,
                             gpointer user_data) {
  ChannelCallbackData* cb = static_cast<ChannelCallbackData*>(user_data);
  if (cb == nullptr || cb->state == nullptr) {
    return;
  }
  JSCValue* js_value =
      js_result != nullptr ? webkit_javascript_result_get_js_value(js_result)
                           : nullptr;
  gchar* msg = js_value != nullptr ? jsc_value_to_string(js_value) : nullptr;
  FlValue* map = fl_value_new_map();
  fl_value_set_string_take(map, "controllerId",
                           fl_value_new_int(cb->controller_id));
  fl_value_set_string_take(map, "channel",
                           fl_value_new_string(cb->channel_name.c_str()));
  fl_value_set_string_take(map, "message",
                           fl_value_new_string(msg != nullptr ? msg : ""));
  g_free(msg);
  SendChannelEvent(cb->state, "onJavaScriptMessage", map);
}

void AddJsChannel(LinuxWebViewInstance* inst, const std::string& name) {
  if (inst == nullptr || inst->ucm == nullptr || name.empty()) {
    return;
  }
  if (inst->channel_signal_ids.find(name) != inst->channel_signal_ids.end()) {
    return;
  }
  std::unique_ptr<ChannelCallbackData> cb(new ChannelCallbackData());
  cb->state = inst->state;
  cb->controller_id = inst->controller_id;
  cb->channel_name = name;
  ChannelCallbackData* raw_cb = cb.get();
  inst->channel_datas.push_back(std::move(cb));

  const std::string detail = "script-message-received::" + name;
  const gulong sig_id = g_signal_connect(
      inst->ucm, detail.c_str(), G_CALLBACK(OnScriptMessageReceived), raw_cb);
  webkit_user_content_manager_register_script_message_handler(inst->ucm,
                                                              name.c_str());
  inst->channel_signal_ids[name] = sig_id;
  RebuildUserScripts(inst);
}

void RemoveJsChannel(LinuxWebViewInstance* inst, const std::string& name) {
  if (inst == nullptr || inst->ucm == nullptr) {
    return;
  }
  std::map<std::string, gulong>::iterator it =
      inst->channel_signal_ids.find(name);
  if (it == inst->channel_signal_ids.end()) {
    return;
  }
  if (it->second != 0) {
    g_signal_handler_disconnect(inst->ucm, it->second);
  }
  webkit_user_content_manager_unregister_script_message_handler(inst->ucm,
                                                                name.c_str());
  inst->channel_signal_ids.erase(it);
  RebuildUserScripts(inst);
}

void ApplyBackgroundColor(LinuxWebViewInstance* inst, uint32_t argb) {
  if (inst == nullptr || inst->webview == nullptr) {
    return;
  }
  GdkRGBA rgba;
  rgba.alpha = static_cast<double>((argb >> 24) & 0xFF) / 255.0;
  rgba.red = static_cast<double>((argb >> 16) & 0xFF) / 255.0;
  rgba.green = static_cast<double>((argb >> 8) & 0xFF) / 255.0;
  rgba.blue = static_cast<double>(argb & 0xFF) / 255.0;
  webkit_web_view_set_background_color(WEBKIT_WEB_VIEW(inst->webview), &rgba);
}

void OnLoadChanged(WebKitWebView* web_view,
                   WebKitLoadEvent load_event,
                   gpointer user_data) {
  LinuxWebViewInstance* inst = static_cast<LinuxWebViewInstance*>(user_data);
  if (inst == nullptr || inst->destroyed) {
    return;
  }
  const gchar* uri = webkit_web_view_get_uri(web_view);
  const char* safe_uri = uri != nullptr ? uri : "";
  if (load_event == WEBKIT_LOAD_STARTED) {
    FlValue* map = fl_value_new_map();
    fl_value_set_string_take(map, "controllerId",
                             fl_value_new_int(inst->controller_id));
    fl_value_set_string_take(map, "url", fl_value_new_string(safe_uri));
    SendChannelEvent(inst->state, "onPageStarted", map);
  } else if (load_event == WEBKIT_LOAD_COMMITTED) {
    if (!inst->in_inline_html_load) {
      inst->loaded_from_html_string = false;
      inst->last_html.clear();
      inst->last_base_url.clear();
      inst->has_last_base_url = false;
    }
  } else if (load_event == WEBKIT_LOAD_FINISHED) {
    inst->in_inline_html_load = false;
    FlValue* map = fl_value_new_map();
    fl_value_set_string_take(map, "controllerId",
                             fl_value_new_int(inst->controller_id));
    fl_value_set_string_take(map, "url", fl_value_new_string(safe_uri));
    SendChannelEvent(inst->state, "onPageFinished", map);
  }
}

gboolean OnLoadFailed(WebKitWebView* /*web_view*/,
                      WebKitLoadEvent /*load_event*/,
                      gchar* failing_uri,
                      GError* error,
                      gpointer user_data) {
  LinuxWebViewInstance* inst = static_cast<LinuxWebViewInstance*>(user_data);
  if (inst == nullptr || inst->destroyed || error == nullptr) {
    return FALSE;
  }
  if (g_error_matches(error, WEBKIT_NETWORK_ERROR,
                      WEBKIT_NETWORK_ERROR_CANCELLED)) {
    return FALSE;
  }
  inst->in_inline_html_load = false;
  FlValue* map = fl_value_new_map();
  fl_value_set_string_take(map, "controllerId",
                           fl_value_new_int(inst->controller_id));
  fl_value_set_string_take(map, "errorCode", fl_value_new_int(error->code));
  fl_value_set_string_take(
      map, "description",
      fl_value_new_string(error->message != nullptr ? error->message
                                                    : "Unknown error"));
  fl_value_set_string_take(map, "errorType", fl_value_new_string("unknown"));
  fl_value_set_string_take(map, "isForMainFrame", fl_value_new_bool(TRUE));
  fl_value_set_string_take(
      map, "url",
      failing_uri != nullptr ? fl_value_new_string(failing_uri)
                             : fl_value_new_null());
  SendChannelEvent(inst->state, "onWebResourceError", map);
  return FALSE;
}

void OnNotifyUri(GObject* object, GParamSpec* /*pspec*/, gpointer user_data) {
  LinuxWebViewInstance* inst = static_cast<LinuxWebViewInstance*>(user_data);
  if (inst == nullptr || inst->destroyed) {
    return;
  }
  const gchar* uri = webkit_web_view_get_uri(WEBKIT_WEB_VIEW(object));
  FlValue* map = fl_value_new_map();
  fl_value_set_string_take(map, "controllerId",
                           fl_value_new_int(inst->controller_id));
  fl_value_set_string_take(map, "url",
                           uri != nullptr ? fl_value_new_string(uri)
                                          : fl_value_new_null());
  SendChannelEvent(inst->state, "onUrlChange", map);
}

void OnNotifyProgress(GObject* object,
                      GParamSpec* /*pspec*/,
                      gpointer user_data) {
  LinuxWebViewInstance* inst = static_cast<LinuxWebViewInstance*>(user_data);
  if (inst == nullptr || inst->destroyed) {
    return;
  }
  const double progress =
      webkit_web_view_get_estimated_load_progress(WEBKIT_WEB_VIEW(object));
  int64_t pct = static_cast<int64_t>(std::round(progress * 100.0));
  if (pct < 0) {
    pct = 0;
  } else if (pct > 100) {
    pct = 100;
  }
  FlValue* map = fl_value_new_map();
  fl_value_set_string_take(map, "controllerId",
                           fl_value_new_int(inst->controller_id));
  fl_value_set_string_take(map, "progress", fl_value_new_int(pct));
  SendChannelEvent(inst->state, "onProgress", map);
}

void OnNavigationDecisionReply(GObject* source_object,
                               GAsyncResult* res,
                               gpointer user_data) {
  std::unique_ptr<PendingPolicyData> pending(
      static_cast<PendingPolicyData*>(user_data));
  bool allow = true;
  GError* error = nullptr;
  FlMethodResponse* response = fl_method_channel_invoke_method_finish(
      FL_METHOD_CHANNEL(source_object), res, &error);
  if (response != nullptr) {
    FlValue* result = fl_method_response_get_result(response, nullptr);
    if (result != nullptr && fl_value_get_type(result) == FL_VALUE_TYPE_BOOL) {
      allow = fl_value_get_bool(result) != FALSE;
    }
    g_object_unref(response);
  }
  if (error != nullptr) {
    g_error_free(error);
  }
  if (pending != nullptr && pending->decision != nullptr) {
    if (allow) {
      webkit_policy_decision_use(pending->decision);
    } else {
      webkit_policy_decision_ignore(pending->decision);
    }
    g_object_unref(pending->decision);
  }
}

gboolean OnDecidePolicy(WebKitWebView* /*web_view*/,
                        WebKitPolicyDecision* decision,
                        WebKitPolicyDecisionType decision_type,
                        gpointer user_data) {
  LinuxWebViewInstance* inst = static_cast<LinuxWebViewInstance*>(user_data);
  if (inst == nullptr || inst->destroyed) {
    return FALSE;
  }
  if (decision_type != WEBKIT_POLICY_DECISION_TYPE_NAVIGATION_ACTION &&
      decision_type != WEBKIT_POLICY_DECISION_TYPE_NEW_WINDOW_ACTION) {
    return FALSE;
  }
  WebKitNavigationPolicyDecision* nav_decision =
      WEBKIT_NAVIGATION_POLICY_DECISION(decision);
  WebKitNavigationAction* action =
      webkit_navigation_policy_decision_get_navigation_action(nav_decision);

  // When an inline HTML document was loaded with a baseUrl, WebKitGTK's
  // native Reload (including the right-click context menu item) attempts to
  // network-fetch baseUrl instead of re-rendering the inline HTML. Intercept
  // reload actions and replay the stored inline HTML in place.
  if (decision_type == WEBKIT_POLICY_DECISION_TYPE_NAVIGATION_ACTION &&
      action != nullptr &&
      webkit_navigation_action_get_navigation_type(action) ==
          WEBKIT_NAVIGATION_TYPE_RELOAD &&
      inst->loaded_from_html_string) {
    webkit_policy_decision_ignore(decision);
    const std::string html_copy = inst->last_html;
    const bool has_base = inst->has_last_base_url;
    const std::string base_copy = inst->last_base_url;
    LoadHtmlOnWebView(inst, html_copy, has_base, base_copy);
    return TRUE;
  }

  if (!inst->has_navigation_decision) {
    return FALSE;
  }

  WebKitURIRequest* req =
      action != nullptr ? webkit_navigation_action_get_request(action)
                        : nullptr;
  const gchar* uri = req != nullptr ? webkit_uri_request_get_uri(req) : nullptr;
  if (uri == nullptr || std::strcmp(uri, "about:blank") == 0) {
    return FALSE;
  }

  const bool is_main_frame =
      decision_type != WEBKIT_POLICY_DECISION_TYPE_NEW_WINDOW_ACTION;
  FlValue* map = fl_value_new_map();
  fl_value_set_string_take(map, "controllerId",
                           fl_value_new_int(inst->controller_id));
  fl_value_set_string_take(map, "url", fl_value_new_string(uri));
  fl_value_set_string_take(map, "isMainFrame",
                           fl_value_new_bool(is_main_frame ? TRUE : FALSE));

  PendingPolicyData* pending = new PendingPolicyData();
  pending->decision = WEBKIT_POLICY_DECISION(g_object_ref(decision));
  fl_method_channel_invoke_method(inst->state->channel, "onNavigationRequest",
                                  map, nullptr, OnNavigationDecisionReply,
                                  pending);
  fl_value_unref(map);
  return TRUE;
}

// Links with target="_blank" and window.open() request a new WebKitWebView.
// Embedded views have no tab strip, so open http(s) targets in place instead of
// silently dropping the click.
GtkWidget* OnCreateWebView(WebKitWebView* web_view,
                           WebKitNavigationAction* navigation_action,
                           gpointer user_data) {
  LinuxWebViewInstance* inst = static_cast<LinuxWebViewInstance*>(user_data);
  if (inst == nullptr || inst->destroyed || navigation_action == nullptr) {
    return nullptr;
  }
  WebKitURIRequest* request =
      webkit_navigation_action_get_request(navigation_action);
  const gchar* uri =
      request != nullptr ? webkit_uri_request_get_uri(request) : nullptr;
  if (uri != nullptr && (g_str_has_prefix(uri, "http://") ||
                         g_str_has_prefix(uri, "https://"))) {
    inst->loaded_from_html_string = false;
    inst->in_inline_html_load = false;
    inst->last_html.clear();
    inst->last_base_url.clear();
    inst->has_last_base_url = false;
    webkit_web_view_load_uri(web_view, uri);
  }
  return nullptr;
}

// Forward function keys (F1-F12) and Super/Meta shortcuts back to FlView so
// application-level shortcuts keep working while WebKitWebView holds GTK focus.
gboolean OnWebViewKeyPress(GtkWidget* /*widget*/,
                           GdkEventKey* event,
                           gpointer user_data) {
  LinuxWebViewInstance* inst = static_cast<LinuxWebViewInstance*>(user_data);
  if (inst == nullptr || inst->destroyed || event == nullptr ||
      inst->state == nullptr || inst->state->registrar == nullptr) {
    return FALSE;
  }
  const bool is_func_key =
      event->keyval >= GDK_KEY_F1 && event->keyval <= GDK_KEY_F12;
  const bool is_super_combo =
      (event->state & (GDK_SUPER_MASK | GDK_META_MASK)) != 0;
  if (!is_func_key && !is_super_combo) {
    return FALSE;
  }
  FlView* fl_view = fl_plugin_registrar_get_view(inst->state->registrar);
  if (fl_view == nullptr) {
    return FALSE;
  }
  GtkWidget* view_widget = GTK_WIDGET(fl_view);
  if (gtk_widget_get_can_focus(view_widget) &&
      !gtk_widget_has_focus(view_widget)) {
    gtk_widget_grab_focus(view_widget);
  }
  gtk_widget_event(view_widget, reinterpret_cast<GdkEvent*>(event));
  return TRUE;
}

void LoadRequestOnWebView(LinuxWebViewInstance* inst,
                          const std::string& url,
                          const std::string& method,
                          FlValue* headers_val,
                          FlValue* body_val) {
  if (inst == nullptr || inst->webview == nullptr) {
    return;
  }
  inst->loaded_from_html_string = false;
  inst->in_inline_html_load = false;
  inst->last_html.clear();
  inst->last_base_url.clear();
  inst->has_last_base_url = false;

  std::string upper_method;
  upper_method.reserve(method.size());
  for (size_t i = 0; i < method.size(); ++i) {
    const unsigned char c = static_cast<unsigned char>(method[i]);
    upper_method.push_back(
        static_cast<char>((c >= 'a' && c <= 'z') ? (c - 'a' + 'A') : c));
  }

  if (upper_method.empty() || upper_method == "GET" || upper_method == "HEAD") {
    WebKitURIRequest* request = webkit_uri_request_new(url.c_str());
    if (headers_val != nullptr &&
        fl_value_get_type(headers_val) == FL_VALUE_TYPE_MAP) {
      SoupMessageHeaders* soup_headers =
          webkit_uri_request_get_http_headers(request);
      if (soup_headers != nullptr) {
        const size_t count = fl_value_get_length(headers_val);
        for (size_t i = 0; i < count; ++i) {
          FlValue* k = fl_value_get_map_key(headers_val, i);
          FlValue* v = fl_value_get_map_value(headers_val, i);
          if (k != nullptr && v != nullptr &&
              fl_value_get_type(k) == FL_VALUE_TYPE_STRING &&
              fl_value_get_type(v) == FL_VALUE_TYPE_STRING) {
            soup_message_headers_replace(soup_headers, fl_value_get_string(k),
                                         fl_value_get_string(v));
          }
        }
      }
    }
    webkit_web_view_load_request(WEBKIT_WEB_VIEW(inst->webview), request);
    g_object_unref(request);
    return;
  }

  // WebKitURIRequest in webkit2gtk-4.1 does not expose a setter for non-GET
  // HTTP methods; execute non-GET navigations via same-origin fetch() with
  // base_uri set to the target URL.
  std::string headers_json = "{";
  if (headers_val != nullptr &&
      fl_value_get_type(headers_val) == FL_VALUE_TYPE_MAP) {
    const size_t count = fl_value_get_length(headers_val);
    bool first = true;
    for (size_t i = 0; i < count; ++i) {
      FlValue* k = fl_value_get_map_key(headers_val, i);
      FlValue* v = fl_value_get_map_value(headers_val, i);
      if (k != nullptr && v != nullptr &&
          fl_value_get_type(k) == FL_VALUE_TYPE_STRING &&
          fl_value_get_type(v) == FL_VALUE_TYPE_STRING) {
        if (!first) {
          headers_json += ",";
        }
        first = false;
        headers_json += EscapeJsonString(fl_value_get_string(k)) + ":" +
                        EscapeJsonString(fl_value_get_string(v));
      }
    }
  }
  headers_json += "}";

  std::string body_expr = "undefined";
  if (body_val != nullptr &&
      fl_value_get_type(body_val) == FL_VALUE_TYPE_UINT8_LIST) {
    const size_t len = fl_value_get_length(body_val);
    const uint8_t* bytes = fl_value_get_uint8_list(body_val);
    if (bytes != nullptr && len > 0) {
      body_expr = "new Uint8Array([";
      for (size_t i = 0; i < len; ++i) {
        if (i > 0) {
          body_expr += ",";
        }
        char num[8];
        snprintf(num, sizeof(num), "%u", static_cast<unsigned int>(bytes[i]));
        body_expr += num;
      }
      body_expr += "])";
    }
  }

  const std::string bootstrap_html =
      "<!DOCTYPE html><html><head><meta charset=\"utf-8\"></head><body><script>"
      "fetch(" +
      EscapeJsonString(url) + ",{method:" + EscapeJsonString(upper_method) +
      ",headers:" + headers_json + ",body:" + body_expr +
      "}).then(function(r){return r.text();}).then(function(t){"
      "document.open();document.write(t);document.close();});"
      "</script></body></html>";
  inst->in_inline_html_load = true;
  webkit_web_view_load_html(WEBKIT_WEB_VIEW(inst->webview),
                            bootstrap_html.c_str(), url.c_str());
}

void OnClearCookiesFinished(GObject* source_object,
                            GAsyncResult* res,
                            gpointer user_data) {
  FlMethodCall* call = FL_METHOD_CALL(user_data);
  GError* error = nullptr;
  const gboolean ok = webkit_website_data_manager_clear_finish(
      WEBKIT_WEBSITE_DATA_MANAGER(source_object), res, &error);
  if (error != nullptr) {
    g_error_free(error);
  }
  FlValue* val = fl_value_new_bool(ok);
  fl_method_call_respond_success(call, val, nullptr);
  fl_value_unref(val);
  g_object_unref(call);
}

void OnAddCookieFinished(GObject* source_object,
                         GAsyncResult* res,
                         gpointer user_data) {
  FlMethodCall* call = FL_METHOD_CALL(user_data);
  GError* error = nullptr;
  webkit_cookie_manager_add_cookie_finish(WEBKIT_COOKIE_MANAGER(source_object),
                                          res, &error);
  if (error != nullptr) {
    g_error_free(error);
  }
  fl_method_call_respond_success(call, nullptr, nullptr);
  g_object_unref(call);
}

#if WEBKIT_CHECK_VERSION(2, 40, 0)
void OnJsEvaluated(GObject* source_object,
                   GAsyncResult* res,
                   gpointer user_data) {
  std::unique_ptr<PendingJsCallData> pending(
      static_cast<PendingJsCallData*>(user_data));
  GError* error = nullptr;
  JSCValue* value = webkit_web_view_evaluate_javascript_finish(
      WEBKIT_WEB_VIEW(source_object), res, &error);
  if (error != nullptr) {
    if (pending->return_result) {
      fl_method_call_respond_error(
          pending->call, "JS_ERROR",
          error->message != nullptr ? error->message : "JavaScript error",
          nullptr, nullptr);
    } else if (pending->is_scroll_pos) {
      FlValue* pos = fl_value_new_map();
      fl_value_set_string_take(pos, "x", fl_value_new_float(0.0));
      fl_value_set_string_take(pos, "y", fl_value_new_float(0.0));
      fl_method_call_respond_success(pending->call, pos, nullptr);
      fl_value_unref(pos);
    } else {
      fl_method_call_respond_success(pending->call, nullptr, nullptr);
    }
    g_error_free(error);
    g_object_unref(pending->call);
    return;
  }

  if (pending->is_scroll_pos) {
    double x = 0.0;
    double y = 0.0;
    if (value != nullptr && jsc_value_is_object(value)) {
      JSCValue* xv = jsc_value_object_get_property_at_index(value, 0);
      JSCValue* yv = jsc_value_object_get_property_at_index(value, 1);
      if (xv != nullptr) {
        x = jsc_value_to_double(xv);
        g_object_unref(xv);
      }
      if (yv != nullptr) {
        y = jsc_value_to_double(yv);
        g_object_unref(yv);
      }
    }
    FlValue* pos = fl_value_new_map();
    fl_value_set_string_take(pos, "x", fl_value_new_float(x));
    fl_value_set_string_take(pos, "y", fl_value_new_float(y));
    fl_method_call_respond_success(pending->call, pos, nullptr);
    fl_value_unref(pos);
  } else if (pending->return_result) {
    FlValue* out = JscValueToFlValue(value);
    fl_method_call_respond_success(pending->call, out, nullptr);
    fl_value_unref(out);
  } else {
    fl_method_call_respond_success(pending->call, nullptr, nullptr);
  }
  if (value != nullptr) {
    g_object_unref(value);
  }
  g_object_unref(pending->call);
}
#else
void OnJsEvaluated(GObject* source_object,
                   GAsyncResult* res,
                   gpointer user_data) {
  std::unique_ptr<PendingJsCallData> pending(
      static_cast<PendingJsCallData*>(user_data));
  GError* error = nullptr;
  WebKitJavascriptResult* js_res = webkit_web_view_run_javascript_finish(
      WEBKIT_WEB_VIEW(source_object), res, &error);
  if (error != nullptr) {
    if (pending->return_result) {
      fl_method_call_respond_error(
          pending->call, "JS_ERROR",
          error->message != nullptr ? error->message : "JavaScript error",
          nullptr, nullptr);
    } else if (pending->is_scroll_pos) {
      FlValue* pos = fl_value_new_map();
      fl_value_set_string_take(pos, "x", fl_value_new_float(0.0));
      fl_value_set_string_take(pos, "y", fl_value_new_float(0.0));
      fl_method_call_respond_success(pending->call, pos, nullptr);
      fl_value_unref(pos);
    } else {
      fl_method_call_respond_success(pending->call, nullptr, nullptr);
    }
    g_error_free(error);
    g_object_unref(pending->call);
    return;
  }

  JSCValue* value =
      js_res != nullptr ? webkit_javascript_result_get_js_value(js_res)
                        : nullptr;
  if (pending->is_scroll_pos) {
    double x = 0.0;
    double y = 0.0;
    if (value != nullptr && jsc_value_is_object(value)) {
      JSCValue* xv = jsc_value_object_get_property_at_index(value, 0);
      JSCValue* yv = jsc_value_object_get_property_at_index(value, 1);
      if (xv != nullptr) {
        x = jsc_value_to_double(xv);
        g_object_unref(xv);
      }
      if (yv != nullptr) {
        y = jsc_value_to_double(yv);
        g_object_unref(yv);
      }
    }
    FlValue* pos = fl_value_new_map();
    fl_value_set_string_take(pos, "x", fl_value_new_float(x));
    fl_value_set_string_take(pos, "y", fl_value_new_float(y));
    fl_method_call_respond_success(pending->call, pos, nullptr);
    fl_value_unref(pos);
  } else if (pending->return_result) {
    FlValue* out = JscValueToFlValue(value);
    fl_method_call_respond_success(pending->call, out, nullptr);
    fl_value_unref(out);
  } else {
    fl_method_call_respond_success(pending->call, nullptr, nullptr);
  }
  if (js_res != nullptr) {
    webkit_javascript_result_unref(js_res);
  }
  g_object_unref(pending->call);
}
#endif

void EvaluateScriptAsync(LinuxWebViewInstance* inst,
                         const std::string& script,
                         FlMethodCall* call,
                         bool return_result,
                         bool is_scroll_pos) {
  if (inst == nullptr || inst->webview == nullptr) {
    if (is_scroll_pos) {
      FlValue* pos = fl_value_new_map();
      fl_value_set_string_take(pos, "x", fl_value_new_float(0.0));
      fl_value_set_string_take(pos, "y", fl_value_new_float(0.0));
      fl_method_call_respond_success(call, pos, nullptr);
      fl_value_unref(pos);
    } else {
      fl_method_call_respond_success(call, nullptr, nullptr);
    }
    return;
  }
  PendingJsCallData* pending = new PendingJsCallData();
  pending->call = FL_METHOD_CALL(g_object_ref(call));
  pending->return_result = return_result;
  pending->is_scroll_pos = is_scroll_pos;
#if WEBKIT_CHECK_VERSION(2, 40, 0)
  webkit_web_view_evaluate_javascript(
      WEBKIT_WEB_VIEW(inst->webview), script.c_str(),
      static_cast<gssize>(script.size()), nullptr, nullptr, nullptr,
      OnJsEvaluated, pending);
#else
  webkit_web_view_run_javascript(WEBKIT_WEB_VIEW(inst->webview), script.c_str(),
                                 nullptr, OnJsEvaluated, pending);
#endif
}

LinuxWebViewInstance* CreateInstance(PluginState* state,
                                     int64_t controller_id,
                                     FlValue* args) {
  state->instances.erase(controller_id);

  std::unique_ptr<LinuxWebViewInstance> inst(new LinuxWebViewInstance());
  inst->state = state;
  inst->controller_id = controller_id;
  inst->ucm = webkit_user_content_manager_new();
  inst->webview = webkit_web_view_new_with_user_content_manager(inst->ucm);
  inst->clip_container = CreateWebviewClipBox(inst->webview);
  gtk_widget_show(inst->webview);

  WebKitSettings* settings =
      webkit_web_view_get_settings(WEBKIT_WEB_VIEW(inst->webview));
  const char* default_ua = webkit_settings_get_user_agent(settings);
  inst->default_user_agent = default_ua != nullptr ? default_ua : "";

  const bool js_enabled = GetBoolArg(args, "javaScriptEnabled", true);
  g_object_set(G_OBJECT(settings), "enable-javascript",
               js_enabled ? TRUE : FALSE, nullptr);

  inst->zoom_enabled = GetBoolArg(args, "zoomEnabled", true);
  inst->has_navigation_decision =
      GetBoolArg(args, "hasNavigationDecision", false);

  bool has_ua = false;
  const std::string custom_ua = GetStringArg(args, "userAgent", &has_ua);
  if (has_ua && !custom_ua.empty()) {
    webkit_settings_set_user_agent(settings, custom_ua.c_str());
  }

  int64_t bg_raw = 0;
  if (GetIntArg(args, "backgroundColor", &bg_raw)) {
    ApplyBackgroundColor(inst.get(), static_cast<uint32_t>(bg_raw));
  } else {
    ApplyBackgroundColor(inst.get(), 0);
  }

  FlValue* channels = LookupMapValue(args, "channelNames");
  if (channels != nullptr &&
      fl_value_get_type(channels) == FL_VALUE_TYPE_LIST) {
    const size_t n = fl_value_get_length(channels);
    for (size_t i = 0; i < n; ++i) {
      FlValue* item = fl_value_get_list_value(channels, i);
      if (item != nullptr && fl_value_get_type(item) == FL_VALUE_TYPE_STRING) {
        const char* cname = fl_value_get_string(item);
        if (cname != nullptr && cname[0] != '\0') {
          AddJsChannel(inst.get(), cname);
        }
      }
    }
  }
  RebuildUserScripts(inst.get());

  inst->load_changed_id = g_signal_connect(
      inst->webview, "load-changed", G_CALLBACK(OnLoadChanged), inst.get());
  inst->load_failed_id = g_signal_connect(inst->webview, "load-failed",
                                          G_CALLBACK(OnLoadFailed), inst.get());
  inst->notify_uri_id = g_signal_connect(inst->webview, "notify::uri",
                                         G_CALLBACK(OnNotifyUri), inst.get());
  inst->notify_progress_id = g_signal_connect(
      inst->webview, "notify::estimated-load-progress",
      G_CALLBACK(OnNotifyProgress), inst.get());
  inst->decide_policy_id = g_signal_connect(
      inst->webview, "decide-policy", G_CALLBACK(OnDecidePolicy), inst.get());
  inst->create_id = g_signal_connect(inst->webview, "create",
                                     G_CALLBACK(OnCreateWebView), inst.get());
  inst->key_press_id = g_signal_connect(
      inst->webview, "key-press-event", G_CALLBACK(OnWebViewKeyPress),
      inst.get());
  inst->webview_destroy_id = g_signal_connect(
      inst->webview, "destroy", G_CALLBACK(OnWebViewDestroyed), inst.get());
  inst->clip_destroy_id =
      g_signal_connect(inst->clip_container, "destroy",
                       G_CALLBACK(OnClipContainerDestroyed), inst.get());

  GtkOverlay* overlay = EnsureOverlay(state);
  if (overlay != nullptr) {
    gtk_overlay_add_overlay(overlay, inst->clip_container);
    SetWebviewClipBoxPassThrough(overlay, inst->clip_container, true);
  }
  gtk_widget_hide(inst->clip_container);

  bool has_html = false;
  const std::string initial_html = GetStringArg(args, "initialHtml", &has_html);
  bool has_url = false;
  const std::string initial_url = GetStringArg(args, "initialUrl", &has_url);

  if (has_html) {
    bool has_base = false;
    const std::string base_url =
        GetStringArg(args, "initialBaseUrl", &has_base);
    LoadHtmlOnWebView(inst.get(), initial_html, has_base, base_url);
  } else if (has_url && !initial_url.empty()) {
    const std::string method = GetStringArg(args, "initialMethod");
    LoadRequestOnWebView(inst.get(), initial_url,
                         method.empty() ? "GET" : method,
                         LookupMapValue(args, "initialHeaders"),
                         LookupMapValue(args, "initialBody"));
  }

  LinuxWebViewInstance* ptr = inst.get();
  state->instances[controller_id] = std::move(inst);
  return ptr;
}

}  // namespace

struct _WebviewCorePlugin {
  GObject parent_instance;
  PluginState* state;
};

G_DEFINE_TYPE(WebviewCorePlugin, webview_core_plugin, G_TYPE_OBJECT)

static void HandleMethodCall(WebviewCorePlugin* self, FlMethodCall* call) {
  PluginState* state = self->state;
  const gchar* method = fl_method_call_get_name(call);
  FlValue* args = fl_method_call_get_args(call);

  if (std::strcmp(method, "clearCookies") == 0) {
    WebKitWebContext* ctx = webkit_web_context_get_default();
    WebKitWebsiteDataManager* mgr =
        webkit_web_context_get_website_data_manager(ctx);
    webkit_website_data_manager_clear(mgr, WEBKIT_WEBSITE_DATA_COOKIES, 0,
                                      nullptr, OnClearCookiesFinished,
                                      g_object_ref(call));
    return;
  }

  if (std::strcmp(method, "setCookie") == 0) {
    const std::string name = GetStringArg(args, "name");
    const std::string value = GetStringArg(args, "value");
    const std::string domain = GetStringArg(args, "domain");
    std::string path = GetStringArg(args, "path");
    if (path.empty()) {
      path = "/";
    }
    if (!name.empty() && !domain.empty()) {
      WebKitWebContext* ctx = webkit_web_context_get_default();
      WebKitCookieManager* cm = webkit_web_context_get_cookie_manager(ctx);
      SoupCookie* cookie = soup_cookie_new(name.c_str(), value.c_str(),
                                           domain.c_str(), path.c_str(), -1);
      if (cookie != nullptr) {
        webkit_cookie_manager_add_cookie(cm, cookie, nullptr,
                                         OnAddCookieFinished,
                                         g_object_ref(call));
        soup_cookie_free(cookie);
        return;
      }
    }
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  int64_t controller_id = 0;
  if (!GetIntArg(args, "controllerId", &controller_id)) {
    fl_method_call_respond_error(call, "INVALID_ARGS", "Missing controllerId",
                                 nullptr, nullptr);
    return;
  }

  if (std::strcmp(method, "create") == 0) {
    CreateInstance(state, controller_id, args);
    FlValue* res = fl_value_new_int(0);
    fl_method_call_respond_success(call, res, nullptr);
    fl_value_unref(res);
    return;
  }

  if (std::strcmp(method, "dispose") == 0) {
    state->instances.erase(controller_id);
    if (state->registrar != nullptr) {
      FlView* fl_view = fl_plugin_registrar_get_view(state->registrar);
      if (fl_view != nullptr) {
        gtk_widget_queue_draw(GTK_WIDGET(fl_view));
      }
    }
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  std::map<int64_t, std::unique_ptr<LinuxWebViewInstance>>::iterator it =
      state->instances.find(controller_id);
  if (it == state->instances.end() || it->second == nullptr ||
      it->second->webview == nullptr || it->second->clip_container == nullptr) {
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }
  LinuxWebViewInstance* inst = it->second.get();

  if (std::strcmp(method, "setGeometry") == 0) {
    const bool visible = GetBoolArg(args, "visible", false);
    const bool pass_through = GetBoolArg(args, "passThrough", false);
    int64_t x = 0;
    int64_t y = 0;
    int64_t w = 0;
    int64_t h = 0;
    GetIntArg(args, "x", &x);
    GetIntArg(args, "y", &y);
    GetIntArg(args, "width", &w);
    GetIntArg(args, "height", &h);

    int64_t full_x = x;
    int64_t full_y = y;
    int64_t full_w = w;
    int64_t full_h = h;
    GetIntArg(args, "fullX", &full_x);
    GetIntArg(args, "fullY", &full_y);
    GetIntArg(args, "fullWidth", &full_w);
    GetIntArg(args, "fullHeight", &full_h);

    inst->x = static_cast<int>(x);
    inst->y = static_cast<int>(y);
    inst->width = static_cast<int>(w);
    inst->height = static_cast<int>(h);
    inst->full_x = static_cast<int>(full_x);
    inst->full_y = static_cast<int>(full_y);
    inst->full_width = static_cast<int>(full_w > 0 ? full_w : w);
    inst->full_height = static_cast<int>(full_h > 0 ? full_h : h);
    inst->visible = visible && w > 0 && h > 0;
    inst->pass_through = !inst->visible || pass_through;

    GtkOverlay* overlay = EnsureOverlay(state);
    if (overlay != nullptr &&
        gtk_widget_get_parent(inst->clip_container) == nullptr) {
      gtk_overlay_add_overlay(overlay, inst->clip_container);
    }

    if (!inst->visible) {
      DismissTransientWebViewPopupsAndGrabs(inst->clip_container,
                                            inst->webview);
      SetWebviewClipBoxPassThrough(overlay, inst->clip_container, true);
      gtk_widget_hide(inst->clip_container);
      RestoreFlViewFocusIfNeeded(state, inst->clip_container);
      if (state->registrar != nullptr) {
        FlView* fl_view = fl_plugin_registrar_get_view(state->registrar);
        if (fl_view != nullptr) {
          gtk_widget_queue_draw(GTK_WIDGET(fl_view));
        }
      }
    } else {
      const int target_w = inst->full_width > 0 ? inst->full_width : 1;
      const int target_h = inst->full_height > 0 ? inst->full_height : 1;
      const int offset_x = inst->full_x - inst->x;
      const int offset_y = inst->full_y - inst->y;
      UpdateWebviewClipBoxChildLayout(inst->clip_container, offset_x, offset_y,
                                      target_w, target_h);
      gtk_widget_set_size_request(inst->webview, target_w, target_h);
      SetWebviewClipBoxPassThrough(overlay, inst->clip_container,
                                   inst->pass_through);
      gtk_widget_queue_allocate(inst->clip_container);
      if (overlay != nullptr) {
        gtk_widget_queue_resize(GTK_WIDGET(overlay));
      }
      gtk_widget_show(inst->clip_container);
    }
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  if (std::strcmp(method, "loadHtmlString") == 0) {
    const std::string html = GetStringArg(args, "html");
    bool has_base = false;
    const std::string base_url = GetStringArg(args, "baseUrl", &has_base);
    LoadHtmlOnWebView(inst, html, has_base, base_url);
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  if (std::strcmp(method, "loadRequest") == 0) {
    const std::string url = GetStringArg(args, "url");
    const std::string req_method = GetStringArg(args, "method");
    LoadRequestOnWebView(inst, url, req_method.empty() ? "GET" : req_method,
                         LookupMapValue(args, "headers"),
                         LookupMapValue(args, "body"));
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  if (std::strcmp(method, "loadFile") == 0) {
    const std::string path = GetStringArg(args, "path");
    gchar* file_uri = g_filename_to_uri(path.c_str(), nullptr, nullptr);
    if (file_uri != nullptr) {
      inst->loaded_from_html_string = false;
      inst->in_inline_html_load = false;
      inst->last_html.clear();
      inst->last_base_url.clear();
      inst->has_last_base_url = false;
      webkit_web_view_load_uri(WEBKIT_WEB_VIEW(inst->webview), file_uri);
      g_free(file_uri);
    }
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  if (std::strcmp(method, "loadFlutterAsset") == 0) {
    FlValue* res = fl_value_new_bool(FALSE);
    fl_method_call_respond_success(call, res, nullptr);
    fl_value_unref(res);
    return;
  }

  if (std::strcmp(method, "setJavaScriptEnabled") == 0) {
    const bool enabled = GetBoolArg(args, "enabled", true);
    WebKitSettings* settings =
        webkit_web_view_get_settings(WEBKIT_WEB_VIEW(inst->webview));
    g_object_set(G_OBJECT(settings), "enable-javascript",
                 enabled ? TRUE : FALSE, nullptr);
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  if (std::strcmp(method, "setBackgroundColor") == 0) {
    int64_t raw = 0;
    GetIntArg(args, "color", &raw);
    ApplyBackgroundColor(inst, static_cast<uint32_t>(raw));
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  if (std::strcmp(method, "setUserAgent") == 0) {
    bool has_ua = false;
    const std::string ua = GetStringArg(args, "userAgent", &has_ua);
    WebKitSettings* settings =
        webkit_web_view_get_settings(WEBKIT_WEB_VIEW(inst->webview));
    if (has_ua && !ua.empty()) {
      webkit_settings_set_user_agent(settings, ua.c_str());
    } else {
      webkit_settings_set_user_agent(settings,
                                     inst->default_user_agent.c_str());
    }
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  if (std::strcmp(method, "enableZoom") == 0) {
    inst->zoom_enabled = GetBoolArg(args, "enabled", true);
    if (!inst->zoom_enabled) {
      webkit_web_view_set_zoom_level(WEBKIT_WEB_VIEW(inst->webview), 1.0);
    }
    RebuildUserScripts(inst);
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  if (std::strcmp(method, "setHasNavigationDecision") == 0) {
    inst->has_navigation_decision = GetBoolArg(args, "enabled", false);
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  if (std::strcmp(method, "runJavaScript") == 0) {
    const std::string script = GetStringArg(args, "script");
    EvaluateScriptAsync(inst, script, call, false, false);
    return;
  }

  if (std::strcmp(method, "runJavaScriptReturningResult") == 0) {
    const std::string script = GetStringArg(args, "script");
    EvaluateScriptAsync(inst, script, call, true, false);
    return;
  }

  if (std::strcmp(method, "addJavaScriptChannel") == 0) {
    AddJsChannel(inst, GetStringArg(args, "name"));
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  if (std::strcmp(method, "removeJavaScriptChannel") == 0) {
    RemoveJsChannel(inst, GetStringArg(args, "name"));
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  if (std::strcmp(method, "reload") == 0) {
    if (inst->loaded_from_html_string) {
      const std::string html_copy = inst->last_html;
      const bool has_base = inst->has_last_base_url;
      const std::string base_copy = inst->last_base_url;
      LoadHtmlOnWebView(inst, html_copy, has_base, base_copy);
    } else {
      webkit_web_view_reload(WEBKIT_WEB_VIEW(inst->webview));
    }
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  if (std::strcmp(method, "stopLoading") == 0) {
    webkit_web_view_stop_loading(WEBKIT_WEB_VIEW(inst->webview));
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  if (std::strcmp(method, "canGoBack") == 0) {
    FlValue* res = fl_value_new_bool(
        webkit_web_view_can_go_back(WEBKIT_WEB_VIEW(inst->webview)));
    fl_method_call_respond_success(call, res, nullptr);
    fl_value_unref(res);
    return;
  }

  if (std::strcmp(method, "canGoForward") == 0) {
    FlValue* res = fl_value_new_bool(
        webkit_web_view_can_go_forward(WEBKIT_WEB_VIEW(inst->webview)));
    fl_method_call_respond_success(call, res, nullptr);
    fl_value_unref(res);
    return;
  }

  if (std::strcmp(method, "goBack") == 0) {
    if (webkit_web_view_can_go_back(WEBKIT_WEB_VIEW(inst->webview))) {
      webkit_web_view_go_back(WEBKIT_WEB_VIEW(inst->webview));
    }
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  if (std::strcmp(method, "goForward") == 0) {
    if (webkit_web_view_can_go_forward(WEBKIT_WEB_VIEW(inst->webview))) {
      webkit_web_view_go_forward(WEBKIT_WEB_VIEW(inst->webview));
    }
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  if (std::strcmp(method, "currentUrl") == 0) {
    const gchar* uri = webkit_web_view_get_uri(WEBKIT_WEB_VIEW(inst->webview));
    FlValue* res =
        uri != nullptr ? fl_value_new_string(uri) : fl_value_new_null();
    fl_method_call_respond_success(call, res, nullptr);
    fl_value_unref(res);
    return;
  }

  if (std::strcmp(method, "getTitle") == 0) {
    const gchar* title =
        webkit_web_view_get_title(WEBKIT_WEB_VIEW(inst->webview));
    FlValue* res =
        title != nullptr ? fl_value_new_string(title) : fl_value_new_null();
    fl_method_call_respond_success(call, res, nullptr);
    fl_value_unref(res);
    return;
  }

  if (std::strcmp(method, "scrollTo") == 0) {
    int64_t x = 0;
    int64_t y = 0;
    GetIntArg(args, "x", &x);
    GetIntArg(args, "y", &y);
    char buf[96];
    snprintf(buf, sizeof(buf), "window.scrollTo(%lld, %lld);",
             static_cast<long long>(x), static_cast<long long>(y));
    EvaluateScriptAsync(inst, buf, call, false, false);
    return;
  }

  if (std::strcmp(method, "scrollBy") == 0) {
    int64_t x = 0;
    int64_t y = 0;
    GetIntArg(args, "x", &x);
    GetIntArg(args, "y", &y);
    char buf[96];
    snprintf(buf, sizeof(buf), "window.scrollBy(%lld, %lld);",
             static_cast<long long>(x), static_cast<long long>(y));
    EvaluateScriptAsync(inst, buf, call, false, false);
    return;
  }

  if (std::strcmp(method, "getScrollPosition") == 0) {
    EvaluateScriptAsync(inst, "[window.scrollX || 0, window.scrollY || 0]",
                        call, false, true);
    return;
  }

  if (std::strcmp(method, "clearCache") == 0) {
    WebKitWebContext* ctx =
        webkit_web_view_get_context(WEBKIT_WEB_VIEW(inst->webview));
    WebKitWebsiteDataManager* mgr =
        webkit_web_context_get_website_data_manager(ctx);
    webkit_website_data_manager_clear(
        mgr,
        static_cast<WebKitWebsiteDataTypes>(WEBKIT_WEBSITE_DATA_DISK_CACHE |
                                            WEBKIT_WEBSITE_DATA_MEMORY_CACHE),
        0, nullptr, nullptr, nullptr);
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  if (std::strcmp(method, "clearLocalStorage") == 0) {
    WebKitWebContext* ctx =
        webkit_web_view_get_context(WEBKIT_WEB_VIEW(inst->webview));
    WebKitWebsiteDataManager* mgr =
        webkit_web_context_get_website_data_manager(ctx);
    webkit_website_data_manager_clear(mgr, WEBKIT_WEBSITE_DATA_LOCAL_STORAGE, 0,
                                      nullptr, nullptr, nullptr);
    fl_method_call_respond_success(call, nullptr, nullptr);
    return;
  }

  fl_method_call_respond_not_implemented(call, nullptr);
}

static void MethodCallCb(FlMethodChannel* /*channel*/,
                         FlMethodCall* method_call,
                         gpointer user_data) {
  WebviewCorePlugin* plugin = static_cast<WebviewCorePlugin*>(user_data);
  HandleMethodCall(plugin, method_call);
}

static void webview_core_plugin_dispose(GObject* object) {
  WebviewCorePlugin* self =
      G_TYPE_CHECK_INSTANCE_CAST(object, webview_core_plugin_get_type(),
                                 WebviewCorePlugin);
  if (self->state != nullptr) {
    self->state->instances.clear();
    if (self->state->tracked_fl_view != nullptr) {
      if (self->state->fl_view_press_id != 0) {
        g_signal_handler_disconnect(self->state->tracked_fl_view,
                                    self->state->fl_view_press_id);
        self->state->fl_view_press_id = 0;
      }
      if (self->state->fl_view_destroy_id != 0) {
        g_signal_handler_disconnect(self->state->tracked_fl_view,
                                    self->state->fl_view_destroy_id);
        self->state->fl_view_destroy_id = 0;
      }
      self->state->tracked_fl_view = nullptr;
    }
    if (self->state->flutter_event_box != nullptr) {
      if (self->state->event_box_press_id != 0) {
        g_signal_handler_disconnect(self->state->flutter_event_box,
                                    self->state->event_box_press_id);
        self->state->event_box_press_id = 0;
      }
      if (self->state->event_box_destroy_id != 0) {
        g_signal_handler_disconnect(self->state->flutter_event_box,
                                    self->state->event_box_destroy_id);
        self->state->event_box_destroy_id = 0;
      }
      self->state->flutter_event_box = nullptr;
    }
    if (self->state->overlay != nullptr) {
      if (self->state->child_pos_handler_id != 0) {
        g_signal_handler_disconnect(self->state->overlay,
                                    self->state->child_pos_handler_id);
        self->state->child_pos_handler_id = 0;
      }
      if (self->state->overlay_size_alloc_id != 0) {
        g_signal_handler_disconnect(self->state->overlay,
                                    self->state->overlay_size_alloc_id);
        self->state->overlay_size_alloc_id = 0;
      }
      if (self->state->overlay_destroy_id != 0) {
        g_signal_handler_disconnect(self->state->overlay,
                                    self->state->overlay_destroy_id);
        self->state->overlay_destroy_id = 0;
      }
      self->state->overlay = nullptr;
    }
    if (self->state->channel != nullptr) {
      fl_method_channel_set_method_call_handler(self->state->channel, nullptr,
                                                nullptr, nullptr);
      g_clear_object(&self->state->channel);
    }
    if (self->state->registrar != nullptr) {
      g_clear_object(&self->state->registrar);
    }
    delete self->state;
    self->state = nullptr;
  }
  G_OBJECT_CLASS(webview_core_plugin_parent_class)->dispose(object);
}

static void webview_core_plugin_class_init(WebviewCorePluginClass* klass) {
  G_OBJECT_CLASS(klass)->dispose = webview_core_plugin_dispose;
}

static void webview_core_plugin_init(WebviewCorePlugin* self) {
  self->state = new PluginState();
}

void webview_core_plugin_register_with_registrar(FlPluginRegistrar* registrar) {
  // Prevent WebKitGTK's DMA-BUF EGL renderer from colliding with Flutter's
  // Wayland EGL subsurface context (which terminates the process with Wayland
  // protocol error 71 on NVIDIA drivers when mapping the first WebKitWebView).
  setenv("WEBKIT_DISABLE_DMABUF_RENDERER", "1", 0);

  WebviewCorePlugin* plugin = static_cast<WebviewCorePlugin*>(
      g_object_new(webview_core_plugin_get_type(), nullptr));

  plugin->state->registrar = FL_PLUGIN_REGISTRAR(g_object_ref(registrar));
  g_autoptr(FlStandardMethodCodec) codec = fl_standard_method_codec_new();
  plugin->state->channel = fl_method_channel_new(
      fl_plugin_registrar_get_messenger(registrar),
      "com.infradise.webview_core", FL_METHOD_CODEC(codec));
  fl_method_channel_set_method_call_handler(plugin->state->channel,
                                            MethodCallCb, g_object_ref(plugin),
                                            g_object_unref);
  g_object_unref(plugin);
}
