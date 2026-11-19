/* Optional Linux Wayland ABI boundary. Protocol objects are owned here;
 * terminal state, input encoding and frame contents remain in no_std Rust. */
#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <limits.h>
#include <locale.h>
#include <poll.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>
#include <wayland-client.h>
#include <wayland-cursor.h>
#include <wayland-egl.h>
#include <xkbcommon/xkbcommon.h>
#include <xkbcommon/xkbcommon-compose.h>
#include "xdg-shell-client-protocol.h"

#define QUEUE_CAPACITY 512
struct ztty_wl_event {
    uint32_t kind, symbol, modifiers, state;
    int32_t x, y;
    char text[64];
};
struct ztty_wl {
    struct wl_display *display;
    struct wl_registry *registry;
    struct wl_compositor *compositor;
    struct wl_shm *shm;
    struct xdg_wm_base *shell;
    struct wl_seat *seat;
    struct wl_keyboard *keyboard;
    struct wl_pointer *pointer;
    struct wl_surface *surface, *cursor_surface;
    struct xdg_surface *xdg_surface;
    struct xdg_toplevel *toplevel;
    struct wl_egl_window *window;
    struct wl_callback *frame;
    struct wl_cursor_theme *cursor_theme;
    struct wl_cursor *cursor;
    struct xkb_context *xkb_context;
    struct xkb_keymap *keymap;
    struct xkb_state *xkb_state;
    struct xkb_compose_table *compose_table;
    struct xkb_compose_state *compose;
    uint32_t compositor_name, shell_name, seat_name;
    int32_t width, height, pending_width, pending_height;
    int32_t pointer_x, pointer_y, axis_discrete;
    double axis_pending, axis_remainder;
    int repeat_rate, repeat_delay;
    uint32_t repeat_key;
    int64_t repeat_at;
    int configured, failed;
    unsigned head, count;
    struct ztty_wl_event events[QUEUE_CAPACITY];
};
static int64_t now_ms(void) {
    struct timespec t;
    if (clock_gettime(CLOCK_MONOTONIC, &t)) return 0;
    return (int64_t)t.tv_sec * 1000 + t.tv_nsec / 1000000;
}
static void push(struct ztty_wl *w, struct ztty_wl_event event) {
    if (w->count == QUEUE_CAPACITY) { w->failed = 1; return; }
    w->events[(w->head + w->count++) % QUEUE_CAPACITY] = event;
}
static uint32_t modifiers(struct ztty_wl *w) {
    if (!w->xkb_state) return 0;
    const char *names[] = {XKB_MOD_NAME_SHIFT, XKB_MOD_NAME_ALT,
        XKB_MOD_NAME_CTRL, XKB_MOD_NAME_LOGO, "Hyper", "Meta",
        XKB_MOD_NAME_CAPS, XKB_MOD_NAME_NUM};
    uint32_t bits = 0;
    for (unsigned i = 0; i < sizeof(names) / sizeof(*names); ++i)
        if (xkb_state_mod_name_is_active(w->xkb_state, names[i],
            XKB_STATE_MODS_EFFECTIVE) > 0) bits |= 1u << i;
    return bits;
}
static void emit_key(struct ztty_wl *w, uint32_t key, uint32_t transition) {
    if (!w->keymap || !w->xkb_state || key > UINT32_MAX - 8) return;
    xkb_keycode_t code = key + 8;
    xkb_keysym_t sym = xkb_state_key_get_one_sym(w->xkb_state, code);
    const xkb_keysym_t *base = NULL;
    xkb_layout_index_t layout = xkb_state_key_get_layout(w->xkb_state, code);
    int count = xkb_keymap_key_get_syms_by_level(w->keymap, code, layout, 0, &base);
    struct ztty_wl_event event = { .kind = 2, .symbol = count == 1 ? base[0] : sym,
        .modifiers = modifiers(w), .state = transition, .x = (int32_t)sym };
    /* Preserve NumLock's keypad interpretation instead of treating KP_0 as
       Insert just because level zero is the navigation keysym. */
    if (sym >= XKB_KEY_KP_Space && sym <= XKB_KEY_KP_Equal) event.symbol = sym;
    int length = xkb_state_key_get_utf8(w->xkb_state, code, event.text, sizeof(event.text));
    if (length < 0 || (size_t)length >= sizeof(event.text)) { w->failed = 1; return; }
    if (transition == 1 && w->compose) {
        xkb_compose_state_feed(w->compose, sym);
        enum xkb_compose_status status = xkb_compose_state_get_status(w->compose);
        if (status == XKB_COMPOSE_COMPOSING) return;
        if (status == XKB_COMPOSE_CANCELLED) {
            xkb_compose_state_reset(w->compose);
            return;
        }
        if (status == XKB_COMPOSE_COMPOSED) {
            length = xkb_compose_state_get_utf8(w->compose, event.text, sizeof(event.text));
            event.symbol = XKB_KEY_NoSymbol;
            xkb_compose_state_reset(w->compose);
            if (length < 0 || (size_t)length >= sizeof(event.text)) { w->failed = 1; return; }
        }
    }
    /* wl_keyboard.modifiers follows modifier key transitions. Encode that
       transition with its resulting state, including when all keys are reported. */
    uint32_t bit = 0;
    if (sym == XKB_KEY_Shift_L || sym == XKB_KEY_Shift_R) bit = 1;
    if (sym == XKB_KEY_Alt_L || sym == XKB_KEY_Alt_R) bit = 2;
    if (sym == XKB_KEY_Control_L || sym == XKB_KEY_Control_R) bit = 4;
    if (sym == XKB_KEY_Super_L || sym == XKB_KEY_Super_R) bit = 8;
    if (transition) event.modifiers |= bit; else event.modifiers &= ~bit;
    push(w, event);
}
static void keyboard_keymap(void *data, struct wl_keyboard *keyboard,
        uint32_t format, int fd, uint32_t size) {
    (void)keyboard;
    struct ztty_wl *w = data;
    w->repeat_at = 0;
    if (format != WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1 || size == 0 || size > 16 * 1024 * 1024) {
        close(fd); w->failed = 1; return;
    }
    char *map = mmap(NULL, size, PROT_READ, MAP_PRIVATE, fd, 0);
    close(fd);
    if (map == MAP_FAILED) { w->failed = 1; return; }
    struct xkb_keymap *keymap = map[size - 1] == 0
        ? xkb_keymap_new_from_string(w->xkb_context, map, XKB_KEYMAP_FORMAT_TEXT_V1,
            XKB_KEYMAP_COMPILE_NO_FLAGS) : NULL;
    munmap(map, size);
    struct xkb_state *state = keymap ? xkb_state_new(keymap) : NULL;
    if (!state) { xkb_keymap_unref(keymap); w->failed = 1; return; }
    xkb_state_unref(w->xkb_state);
    xkb_keymap_unref(w->keymap);
    w->xkb_state = state; w->keymap = keymap;
    if (w->compose) xkb_compose_state_reset(w->compose);
}
static void keyboard_enter(void *data, struct wl_keyboard *keyboard,
        uint32_t serial, struct wl_surface *surface, struct wl_array *keys) {
    (void)keyboard; (void)serial; (void)surface; (void)keys;
    push(data, (struct ztty_wl_event){.kind = 3, .state = 1});
}
static void keyboard_leave(void *data, struct wl_keyboard *keyboard,
        uint32_t serial, struct wl_surface *surface) {
    (void)keyboard; (void)serial; (void)surface;
    struct ztty_wl *w = data; w->repeat_at = 0;
    if (w->compose) xkb_compose_state_reset(w->compose);
    push(w, (struct ztty_wl_event){.kind = 3});
}
static void keyboard_key(void *data, struct wl_keyboard *keyboard,
        uint32_t serial, uint32_t time, uint32_t key, uint32_t state) {
    (void)keyboard; (void)serial; (void)time;
    struct ztty_wl *w = data;
    emit_key(w, key, state == WL_KEYBOARD_KEY_STATE_PRESSED ? 1 : 0);
    if (state == WL_KEYBOARD_KEY_STATE_PRESSED && w->repeat_rate > 0 &&
        w->keymap && key <= UINT32_MAX - 8 && xkb_keymap_key_repeats(w->keymap, key + 8)) {
        w->repeat_key = key; w->repeat_at = now_ms() + w->repeat_delay;
    } else if (key == w->repeat_key) w->repeat_at = 0;
}
static void keyboard_modifiers(void *data, struct wl_keyboard *keyboard,
        uint32_t serial, uint32_t depressed, uint32_t latched, uint32_t locked, uint32_t group) {
    (void)keyboard; (void)serial;
    struct ztty_wl *w = data;
    if (w->xkb_state) xkb_state_update_mask(w->xkb_state, depressed, latched, locked, 0, 0, group);
}
static void keyboard_repeat(void *data, struct wl_keyboard *keyboard, int32_t rate, int32_t delay) {
    (void)keyboard; struct ztty_wl *w = data;
    w->repeat_rate = rate > 1000 ? 1000 : rate > 0 ? rate : 0;
    w->repeat_delay = delay > 60000 ? 60000 : delay > 0 ? delay : 0;
    if (!w->repeat_rate) w->repeat_at = 0;
}
static const struct wl_keyboard_listener keyboard_listener = {
    .keymap = keyboard_keymap, .enter = keyboard_enter, .leave = keyboard_leave,
    .key = keyboard_key, .modifiers = keyboard_modifiers, .repeat_info = keyboard_repeat,
};
static void pointer_enter(void *data, struct wl_pointer *pointer, uint32_t serial,
        struct wl_surface *surface, wl_fixed_t x, wl_fixed_t y) {
    (void)surface; struct ztty_wl *w = data;
    w->pointer_x = wl_fixed_to_int(x); w->pointer_y = wl_fixed_to_int(y);
    if (w->cursor && w->cursor_surface) {
        struct wl_cursor_image *image = w->cursor->images[0];
        wl_pointer_set_cursor(pointer, serial, w->cursor_surface, image->hotspot_x, image->hotspot_y);
        wl_surface_attach(w->cursor_surface, wl_cursor_image_get_buffer(image), 0, 0);
        wl_surface_damage(w->cursor_surface, 0, 0, INT32_MAX, INT32_MAX);
        wl_surface_commit(w->cursor_surface);
    }
}
static void pointer_leave(void *data, struct wl_pointer *pointer,
        uint32_t serial, struct wl_surface *surface) {
    (void)data; (void)pointer; (void)serial; (void)surface;
}
static void pointer_motion(void *data, struct wl_pointer *pointer, uint32_t time, wl_fixed_t x, wl_fixed_t y) {
    (void)pointer; (void)time; struct ztty_wl *w = data;
    w->pointer_x = wl_fixed_to_int(x); w->pointer_y = wl_fixed_to_int(y);
    push(w, (struct ztty_wl_event){.kind = 7, .modifiers = modifiers(w), .x = w->pointer_x, .y = w->pointer_y});
}
static void pointer_button(void *data, struct wl_pointer *pointer,
        uint32_t serial, uint32_t time, uint32_t button, uint32_t state) {
    (void)pointer; (void)serial; (void)time; struct ztty_wl *w = data;
    push(w, (struct ztty_wl_event){.kind = 4, .symbol = button, .state = state,
        .modifiers = modifiers(w), .x = w->pointer_x, .y = w->pointer_y});
}
static void pointer_axis(void *data, struct wl_pointer *pointer,
        uint32_t time, uint32_t axis, wl_fixed_t value) {
    (void)pointer; (void)time;
    if (axis == WL_POINTER_AXIS_VERTICAL_SCROLL) ((struct ztty_wl *)data)->axis_pending += wl_fixed_to_double(value);
}
static void pointer_frame(void *data, struct wl_pointer *pointer) {
    (void)pointer; struct ztty_wl *w = data;
    double lines = w->axis_discrete ? w->axis_discrete : (w->axis_remainder + w->axis_pending) / 10.0;
    int steps = lines > 64 ? 64 : lines < -64 ? -64 : (int)lines;
    w->axis_remainder = w->axis_discrete ? 0 : (lines - steps) * 10.0;
    w->axis_discrete = 0; w->axis_pending = 0;
    if (steps) push(w, (struct ztty_wl_event){.kind = 5, .state = (uint32_t)steps,
        .modifiers = modifiers(w), .x = w->pointer_x, .y = w->pointer_y});
}
static void pointer_source(void *data, struct wl_pointer *pointer, uint32_t source) {
    (void)data; (void)pointer; (void)source;
}
static void pointer_stop(void *data, struct wl_pointer *pointer, uint32_t time, uint32_t axis) {
    (void)pointer; (void)time;
    if (axis == WL_POINTER_AXIS_VERTICAL_SCROLL) ((struct ztty_wl *)data)->axis_remainder = 0;
}
static void pointer_discrete(void *data, struct wl_pointer *pointer, uint32_t axis, int32_t discrete) {
    (void)pointer;
    struct ztty_wl *w = data;
    if (axis == WL_POINTER_AXIS_VERTICAL_SCROLL) {
        int64_t value = (int64_t)w->axis_discrete + discrete;
        w->axis_discrete = value > 64 ? 64 : value < -64 ? -64 : (int32_t)value;
    }
}
static const struct wl_pointer_listener pointer_listener = {
    .enter = pointer_enter, .leave = pointer_leave, .motion = pointer_motion,
    .button = pointer_button, .axis = pointer_axis, .frame = pointer_frame,
    .axis_source = pointer_source, .axis_stop = pointer_stop, .axis_discrete = pointer_discrete,
};
static void seat_capabilities(void *data, struct wl_seat *seat, uint32_t caps) {
    struct ztty_wl *w = data;
    if ((caps & WL_SEAT_CAPABILITY_KEYBOARD) && !w->keyboard) {
        w->keyboard = wl_seat_get_keyboard(seat);
        wl_keyboard_add_listener(w->keyboard, &keyboard_listener, w);
    } else if (!(caps & WL_SEAT_CAPABILITY_KEYBOARD) && w->keyboard) {
        wl_keyboard_release(w->keyboard); w->keyboard = NULL; w->repeat_at = 0;
        push(w, (struct ztty_wl_event){.kind = 3});
    }
    if ((caps & WL_SEAT_CAPABILITY_POINTER) && !w->pointer) {
        w->pointer = wl_seat_get_pointer(seat);
        wl_pointer_add_listener(w->pointer, &pointer_listener, w);
    } else if (!(caps & WL_SEAT_CAPABILITY_POINTER) && w->pointer) {
        wl_pointer_release(w->pointer); w->pointer = NULL;
    }
}
static void seat_name(void *data, struct wl_seat *seat, const char *name) {
    (void)data; (void)seat; (void)name;
}
static const struct wl_seat_listener seat_listener = {.capabilities = seat_capabilities, .name = seat_name};
static void shell_ping(void *data, struct xdg_wm_base *shell, uint32_t serial) {
    (void)data; xdg_wm_base_pong(shell, serial);
}
static const struct xdg_wm_base_listener shell_listener = {.ping = shell_ping};
static void shm_format(void *data, struct wl_shm *shm, uint32_t format) {
    (void)data; (void)shm; (void)format;
}
static const struct wl_shm_listener shm_listener = {.format = shm_format};
static void registry_global(void *data, struct wl_registry *registry,
        uint32_t name, const char *interface, uint32_t version) {
    struct ztty_wl *w = data;
    if (!strcmp(interface, wl_compositor_interface.name) && !w->compositor) {
        w->compositor = wl_registry_bind(registry, name, &wl_compositor_interface, version < 4 ? version : 4);
        w->compositor_name = name;
    } else if (!strcmp(interface, xdg_wm_base_interface.name) && !w->shell) {
        w->shell = wl_registry_bind(registry, name, &xdg_wm_base_interface, 1);
        w->shell_name = name; xdg_wm_base_add_listener(w->shell, &shell_listener, w);
    } else if (!strcmp(interface, wl_seat_interface.name) && !w->seat && version >= 5) {
        w->seat = wl_registry_bind(registry, name, &wl_seat_interface, 5);
        w->seat_name = name; wl_seat_add_listener(w->seat, &seat_listener, w);
    } else if (!strcmp(interface, wl_shm_interface.name) && !w->shm) {
        w->shm = wl_registry_bind(registry, name, &wl_shm_interface, 1);
        wl_shm_add_listener(w->shm, &shm_listener, w);
    }
}
static void registry_remove(void *data, struct wl_registry *registry, uint32_t name) {
    (void)registry; struct ztty_wl *w = data;
    if (name == w->compositor_name || name == w->shell_name) w->failed = 1;
    if (name == w->seat_name) {
        if (w->keyboard) wl_keyboard_release(w->keyboard);
        if (w->pointer) wl_pointer_release(w->pointer);
        if (w->seat) wl_seat_release(w->seat);
        w->keyboard = NULL; w->pointer = NULL; w->seat = NULL; w->repeat_at = 0;
        push(w, (struct ztty_wl_event){.kind = 3});
    }
}
static const struct wl_registry_listener registry_listener = {.global = registry_global, .global_remove = registry_remove};
static void toplevel_configure(void *data, struct xdg_toplevel *toplevel,
        int32_t width, int32_t height, struct wl_array *states) {
    (void)toplevel; (void)states; struct ztty_wl *w = data;
    if (width > 0) w->pending_width = width;
    if (height > 0) w->pending_height = height;
}
static void toplevel_close(void *data, struct xdg_toplevel *toplevel) {
    (void)toplevel; push(data, (struct ztty_wl_event){.kind = 6});
}
static const struct xdg_toplevel_listener toplevel_listener = {.configure = toplevel_configure, .close = toplevel_close};
static void surface_configure(void *data, struct xdg_surface *surface, uint32_t serial) {
    struct ztty_wl *w = data;
    xdg_surface_ack_configure(surface, serial);
    w->width = w->pending_width; w->height = w->pending_height; w->configured = 1;
    push(w, (struct ztty_wl_event){.kind = 1, .x = w->width, .y = w->height});
}
static const struct xdg_surface_listener surface_listener = {.configure = surface_configure};
static void frame_done(void *data, struct wl_callback *callback, uint32_t time) {
    (void)time; struct ztty_wl *w = data; wl_callback_destroy(callback); w->frame = NULL;
}
static const struct wl_callback_listener frame_listener = {.done = frame_done};

void ztty_wl_destroy(struct ztty_wl *w) {
    if (!w) return;
    if (w->frame) wl_callback_destroy(w->frame);
    if (w->window) wl_egl_window_destroy(w->window);
    if (w->toplevel) xdg_toplevel_destroy(w->toplevel);
    if (w->xdg_surface) xdg_surface_destroy(w->xdg_surface);
    if (w->surface) wl_surface_destroy(w->surface);
    if (w->keyboard) wl_keyboard_release(w->keyboard);
    if (w->pointer) wl_pointer_release(w->pointer);
    if (w->seat) wl_seat_release(w->seat);
    if (w->cursor_surface) wl_surface_destroy(w->cursor_surface);
    if (w->cursor_theme) wl_cursor_theme_destroy(w->cursor_theme);
    if (w->shm) wl_shm_destroy(w->shm);
    if (w->shell) xdg_wm_base_destroy(w->shell);
    if (w->compositor) wl_compositor_destroy(w->compositor);
    if (w->registry) wl_registry_destroy(w->registry);
    xkb_compose_state_unref(w->compose); xkb_compose_table_unref(w->compose_table);
    xkb_state_unref(w->xkb_state); xkb_keymap_unref(w->keymap); xkb_context_unref(w->xkb_context);
    if (w->display) wl_display_disconnect(w->display);
    free(w);
}
struct ztty_wl *ztty_wl_create(int32_t width, int32_t height) {
    struct ztty_wl *w = calloc(1, sizeof(*w));
    if (!w) return NULL;
    w->width = w->pending_width = width; w->height = w->pending_height = height;
    w->display = wl_display_connect(NULL);
    w->xkb_context = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
    if (!w->display || !w->xkb_context) goto fail;
    w->compose_table = xkb_compose_table_new_from_locale(w->xkb_context,
        setlocale(LC_CTYPE, NULL), XKB_COMPOSE_COMPILE_NO_FLAGS);
    if (w->compose_table) w->compose = xkb_compose_state_new(w->compose_table, XKB_COMPOSE_STATE_NO_FLAGS);
    w->registry = wl_display_get_registry(w->display);
    if (!w->registry) goto fail;
    wl_registry_add_listener(w->registry, &registry_listener, w);
    if (wl_display_roundtrip(w->display) < 0 || !w->compositor || !w->shell || w->failed) goto fail;
    w->surface = wl_compositor_create_surface(w->compositor);
    if (!w->surface) goto fail;
    w->xdg_surface = xdg_wm_base_get_xdg_surface(w->shell, w->surface);
    if (!w->xdg_surface) goto fail;
    xdg_surface_add_listener(w->xdg_surface, &surface_listener, w);
    w->toplevel = xdg_surface_get_toplevel(w->xdg_surface);
    if (!w->toplevel) goto fail;
    xdg_toplevel_add_listener(w->toplevel, &toplevel_listener, w);
    xdg_toplevel_set_title(w->toplevel, "ztty");
    xdg_toplevel_set_app_id(w->toplevel, "org.project-z.ztty");
    xdg_toplevel_set_min_size(w->toplevel, 32, 32);
    w->window = wl_egl_window_create(w->surface, width, height);
    if (!w->window) goto fail;
    if (w->shm) {
        w->cursor_theme = wl_cursor_theme_load(NULL, 24, w->shm);
        if (w->cursor_theme) w->cursor = wl_cursor_theme_get_cursor(w->cursor_theme, "left_ptr");
        if (w->cursor && w->cursor->image_count) w->cursor_surface = wl_compositor_create_surface(w->compositor);
        else w->cursor = NULL;
    }
    wl_surface_commit(w->surface); /* No buffer before xdg_surface.configure. */
    return w;
fail:
    ztty_wl_destroy(w); return NULL;
}
void *ztty_wl_display(struct ztty_wl *w) { return w->display; }
uintptr_t ztty_wl_window(struct ztty_wl *w) { return (uintptr_t)w->window; }
int ztty_wl_next(struct ztty_wl *w, struct ztty_wl_event *event) {
    if (!w->count) return 0;
    *event = w->events[w->head]; w->head = (w->head + 1) % QUEUE_CAPACITY; --w->count;
    return 1;
}
int ztty_wl_resize(struct ztty_wl *w, int width, int height) {
    if (width < 1 || height < 1 || width > 8192 || height > 8192 ||
        (int64_t)width * height > 16777216) return -1;
    wl_egl_window_resize(w->window, width, height, 0, 0);
    xdg_surface_set_window_geometry(w->xdg_surface, 0, 0, width, height);
    return 0;
}
void ztty_wl_title(struct ztty_wl *w, const char *title) { xdg_toplevel_set_title(w->toplevel, title); }
int ztty_wl_ready(struct ztty_wl *w) { return w->configured && !w->frame && !w->failed; }
int ztty_wl_request_frame(struct ztty_wl *w) {
    if (!ztty_wl_ready(w)) return -1;
    w->frame = wl_surface_frame(w->surface);
    if (!w->frame) return -1;
    wl_callback_add_listener(w->frame, &frame_listener, w);
    return 0;
}
/* Follow prepare_read -> poll -> read_events/cancel_read on every path, while
 * polling the PTY in the same wait. Never dispatch() after a prior fd check:
 * that can block if EGL already consumed the readable socket event. */
int ztty_wl_pump(struct ztty_wl *w, int pty_fd, int want_write, int timeout) {
    if (w->failed) return -1;
    while (wl_display_prepare_read(w->display) != 0) {
        if (wl_display_dispatch_pending(w->display) < 0 || w->failed) return -1;
    }
    int flush = wl_display_flush(w->display);
    if (flush < 0 && errno != EAGAIN) { wl_display_cancel_read(w->display); return -1; }
    int64_t now = now_ms();
    if (w->count) timeout = 0;
    if (w->repeat_at) {
        int64_t remaining = w->repeat_at > now ? w->repeat_at - now : 0;
        if (timeout < 0 || remaining < timeout) timeout = (int)remaining;
    }
    struct pollfd fds[] = {
        {.fd = wl_display_get_fd(w->display), .events = POLLIN | (flush < 0 ? POLLOUT : 0)},
        {.fd = pty_fd, .events = POLLIN | (want_write ? POLLOUT : 0)},
    };
    int result = poll(fds, 2, timeout);
    if (result < 0) {
        wl_display_cancel_read(w->display);
        return errno == EINTR ? 0 : -1;
    }
    if (fds[0].revents & (POLLERR | POLLHUP | POLLNVAL)) { wl_display_cancel_read(w->display); return -1; }
    if (fds[0].revents & POLLIN) {
        if (wl_display_read_events(w->display) < 0) return -1;
    } else wl_display_cancel_read(w->display);
    if ((fds[0].revents & POLLOUT) && wl_display_flush(w->display) < 0 && errno != EAGAIN) return -1;
    if (wl_display_dispatch_pending(w->display) < 0 || w->failed) return -1;
    now = now_ms();
    if (w->repeat_at && w->repeat_at <= now) {
        emit_key(w, w->repeat_key, 2);
        w->repeat_at = now + (1000 / w->repeat_rate > 0 ? 1000 / w->repeat_rate : 1);
    }
    return w->failed ? -1 : fds[1].revents;
}
